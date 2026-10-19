//! Persian pronunciation (G2P) for Pocket TTS Farsi v2: Homo-GE2PE-Persian-HF
//! (tiny ByT5) as encoder + KV-cache decoder ONNX, embedded in the binary.
//!
//! Rust port of readly's `G2p.kt`: one cleaned Persian sentence goes in, one
//! phoneme string comes out, via the same beam search `transformers` runs
//! for `generate(num_beams=5, max_length=512, early_stopping=True)` (length
//! penalty 1.0): the encoder computes each decoder layer's cross-attention
//! K/V once, and every step feeds one token plus the past self K/V,
//! reordered by the beam selection like HF's cache. Checked against the
//! reference on every golden sentence
//! (`tests/fixtures/pocket/v2-golden.jsonl`).

use std::borrow::Cow;

use ort::session::{Session, SessionInputValue, SessionInputs};

use crate::engine::EngineError;

use super::runtime::{ort_environment, tensor_f32, tensor_i64};

const PAD_ID: i64 = 0;
const EOS_ID: i64 = 1;
const BYTE_OFFSET: i64 = 3;
const FIRST_BYTE_ID: i64 = 3;
const LAST_BYTE_ID: i64 = 258;
const VOCAB_SIZE: usize = 384;
const NUM_HEADS: usize = 6;
const D_KV: usize = 64;
const NUM_BEAMS: usize = 5;
const BEAMS_TO_KEEP: usize = 10;
const MAX_LENGTH: usize = 512;
const LENGTH_PENALTY: f64 = 1.0;
// GE2PE config (relative_attention_num_buckets/max_distance); the golden
// test guards these (every step's bias comes from them).
const NUM_BUCKETS: usize = 32;
const MAX_DISTANCE: usize = 128;

const ENCODER_BYTES: &[u8] = include_bytes!("../../assets/ge2pe/encoder.onnx");
const DECODER_BYTES: &[u8] = include_bytes!("../../assets/ge2pe/decoder.onnx");

/// Persian pronunciation model (G2P) for Pocket v2 voices.
///
/// Not thread-safe: every call must come from one owner thread. The
/// [`PocketEngine`](super::PocketEngine) owns it for v2 voices.
pub struct G2p {
    encoder: Session,
    decoder: Session,
}

impl G2p {
    /// Load the pronunciation model from the bytes embedded in the binary.
    /// Each session takes one core: a single intra-op and inter-op thread.
    pub fn load() -> Result<Self, EngineError> {
        Ok(Self {
            encoder: open_session(ENCODER_BYTES)?,
            decoder: open_session(DECODER_BYTES)?,
        })
    }

    /// Pronounce one cleaned Persian sentence (reference
    /// `clean_sentence_for_g2p` output): beam search over the decoder, ByT5
    /// byte ids, specials skipped. Empty input returns empty output.
    pub fn phonemise(&mut self, clean: &str) -> Result<String, EngineError> {
        if clean.is_empty() {
            return Ok(String::new());
        }
        self.phonemise_inner(clean)
            .map_err(|err| EngineError::G2pFailed {
                sentence: clean.to_string(),
                message: match err {
                    EngineError::PocketData(message) => message,
                    other => other.to_string(),
                },
            })
    }

    fn phonemise_inner(&mut self, clean: &str) -> Result<String, EngineError> {
        let ids = encode_bytes(clean);
        let cross = self.run_encoder(&ids)?;
        let best = self.search(&cross)?;
        Ok(decode_bytes(&best))
    }

    fn run_encoder(&mut self, ids: &[i64]) -> Result<CrossKv, EngineError> {
        let inputs: Vec<(Cow<'_, str>, SessionInputValue<'_>)> = vec![(
            Cow::Borrowed("input_ids"),
            tensor_i64(vec![1, ids.len()], ids.to_vec())?.into(),
        )];
        let outputs = self
            .encoder
            .run(SessionInputs::<'_, '_, 0>::ValueMap(inputs))
            .map_err(|err| EngineError::PocketData(format!("model step failed: {err}")))?;
        let k0 = extract_f32(&outputs, "cross_k_0")?;
        let enc_len = k0.len() / NUM_HEADS / D_KV;
        Ok(CrossKv {
            k0,
            v0: extract_f32(&outputs, "cross_v_0")?,
            k1: extract_f32(&outputs, "cross_k_1")?,
            v1: extract_f32(&outputs, "cross_v_1")?,
            enc_len,
        })
    }

    fn search(&mut self, cross: &CrossKv) -> Result<Vec<i64>, EngineError> {
        // Every beam reads the same cross K/V; repeat once up front.
        let cross_shape = vec![NUM_BEAMS, NUM_HEADS, cross.enc_len, D_KV];
        let cross_k0 = tensor_f32(cross_shape.clone(), repeat_beams(&cross.k0))?;
        let cross_v0 = tensor_f32(cross_shape.clone(), repeat_beams(&cross.v0))?;
        let cross_k1 = tensor_f32(cross_shape.clone(), repeat_beams(&cross.k1))?;
        let cross_v1 = tensor_f32(cross_shape.clone(), repeat_beams(&cross.v1))?;
        // Past self K/V per layer, [beam, head, past, d], grown by the
        // decoder each step and reordered by the beam selection.
        let mut past_k0: Vec<f32> = Vec::new();
        let mut past_v0: Vec<f32> = Vec::new();
        let mut past_k1: Vec<f32> = Vec::new();
        let mut past_v1: Vec<f32> = Vec::new();
        let mut running: Vec<Vec<i64>> = vec![vec![PAD_ID]; NUM_BEAMS];
        let mut running_scores: Vec<f64> = (0..NUM_BEAMS)
            .map(|b| if b == 0 { 0.0 } else { -1e9 })
            .collect();
        let mut finalized: Vec<(Vec<i64>, f64, bool)> =
            vec![(vec![PAD_ID], -1e9, false); NUM_BEAMS];
        let mut cur_len = 1usize;
        let mut heuristic_ok = true;
        loop {
            let past_len = cur_len - 1;
            let last_ids: Vec<i64> = running
                .iter()
                .map(|seq| *seq.last().expect("running beams are never empty"))
                .collect();
            let past_shape = vec![NUM_BEAMS, NUM_HEADS, past_len, D_KV];
            let inputs: Vec<(Cow<'_, str>, SessionInputValue<'_>)> = vec![
                (
                    Cow::Borrowed("decoder_input_ids"),
                    tensor_i64(vec![NUM_BEAMS, 1], last_ids)?.into(),
                ),
                (
                    Cow::Borrowed("self_bias_buckets"),
                    tensor_i64(vec![past_len + 1], self_bias_buckets(past_len))?.into(),
                ),
                (
                    Cow::Borrowed("past_k_0"),
                    tensor_f32(past_shape.clone(), past_k0.clone())?.into(),
                ),
                (
                    Cow::Borrowed("past_v_0"),
                    tensor_f32(past_shape.clone(), past_v0.clone())?.into(),
                ),
                (
                    Cow::Borrowed("past_k_1"),
                    tensor_f32(past_shape.clone(), past_k1.clone())?.into(),
                ),
                (
                    Cow::Borrowed("past_v_1"),
                    tensor_f32(past_shape.clone(), past_v1.clone())?.into(),
                ),
                (Cow::Borrowed("cross_k_0"), (&cross_k0).into()),
                (Cow::Borrowed("cross_v_0"), (&cross_v0).into()),
                (Cow::Borrowed("cross_k_1"), (&cross_k1).into()),
                (Cow::Borrowed("cross_v_1"), (&cross_v1).into()),
            ];
            let outputs = self
                .decoder
                .run(SessionInputs::<'_, '_, 0>::ValueMap(inputs))
                .map_err(|err| EngineError::PocketData(format!("model step failed: {err}")))?;
            let logits = extract_f32(&outputs, "logits")?;
            let present = [
                extract_f32(&outputs, "present_k_0")?,
                extract_f32(&outputs, "present_v_0")?,
                extract_f32(&outputs, "present_k_1")?,
                extract_f32(&outputs, "present_v_1")?,
            ];
            let pick = select_beams(
                &logits,
                &running,
                &running_scores,
                &finalized,
                heuristic_ok,
                cur_len,
            );
            running = pick.seqs;
            running_scores = pick.scores;
            finalized = pick.finalized;
            heuristic_ok = pick.heuristic_ok;
            // Reorder the grown cache by the selected beams.
            past_k0 = gather_beams(&present[0], &pick.beams, past_len + 1);
            past_v0 = gather_beams(&present[1], &pick.beams, past_len + 1);
            past_k1 = gather_beams(&present[2], &pick.beams, past_len + 1);
            past_v1 = gather_beams(&present[3], &pick.beams, past_len + 1);
            cur_len += 1;
            if !pick.keep_going {
                break;
            }
        }
        // First maximum on ties, like `maxByOrNull` (`max_by` takes the last).
        let mut best_idx = 0;
        for (i, entry) in finalized.iter().enumerate().skip(1) {
            if entry.1 > finalized[best_idx].1 {
                best_idx = i;
            }
        }
        let best = &finalized[best_idx].0;
        let end = best
            .iter()
            .position(|&id| id == EOS_ID)
            .unwrap_or(best.len());
        Ok(best[..end]
            .iter()
            .copied()
            .filter(|&id| id != PAD_ID)
            .collect())
    }
}

fn open_session(bytes: &[u8]) -> Result<Session, EngineError> {
    let env = ort_environment()?;
    Session::builder(env)
        .map_err(|err| EngineError::G2pLoadFailed(err.to_string()))?
        .with_intra_threads(1)
        .map_err(|err| EngineError::G2pLoadFailed(err.to_string()))?
        .with_inter_threads(1)
        .map_err(|err| EngineError::G2pLoadFailed(err.to_string()))?
        .commit_from_memory(bytes)
        .map_err(|err| EngineError::G2pLoadFailed(err.to_string()))
}

fn extract_f32(
    outputs: &ort::session::SessionOutputs<'_>,
    name: &str,
) -> Result<Vec<f32>, EngineError> {
    let value = outputs.get(name).ok_or_else(|| {
        EngineError::PocketData(format!("Pronunciation model gave no '{name}' output"))
    })?;
    let (_, data) = value.try_extract_tensor::<f32>().map_err(|err| {
        EngineError::PocketData(format!("Pronunciation model output '{name}': {err}"))
    })?;
    Ok(data.to_vec())
}

/// Per-layer cross-attention K/V for one sentence (batch 1).
struct CrossKv {
    k0: Vec<f32>,
    v0: Vec<f32>,
    k1: Vec<f32>,
    v1: Vec<f32>,
    enc_len: usize,
}

/// One beam-selection round over a step's logits (HF `_beam_search`).
struct BeamPick {
    seqs: Vec<Vec<i64>>,
    scores: Vec<f64>,
    beams: Vec<usize>,
    finalized: Vec<(Vec<i64>, f64, bool)>,
    heuristic_ok: bool,
    keep_going: bool,
}

fn select_beams(
    logits: &[f32],
    running: &[Vec<i64>],
    running_scores: &[f64],
    finalized: &[(Vec<i64>, f64, bool)],
    heuristic_ok: bool,
    cur_len: usize,
) -> BeamPick {
    // Log-softmax of each beam's one position, plus its score.
    let mut flat = vec![0.0f64; NUM_BEAMS * VOCAB_SIZE];
    for (b, &score) in running_scores.iter().enumerate() {
        let base = b * VOCAB_SIZE;
        let mut max = f64::NEG_INFINITY;
        for v in 0..VOCAB_SIZE {
            let x = f64::from(logits[base + v]);
            if x > max {
                max = x;
            }
        }
        let mut sum = 0.0;
        for v in 0..VOCAB_SIZE {
            sum += (f64::from(logits[base + v]) - max).exp();
        }
        let log_sum = sum.ln();
        for v in 0..VOCAB_SIZE {
            flat[base + v] = f64::from(logits[base + v]) - max - log_sum + score;
        }
    }
    // Stable top-K: score desc, flat index asc.
    let mut order: Vec<usize> = (0..flat.len()).collect();
    order.sort_by(|&a, &b| flat[b].total_cmp(&flat[a]).then(a.cmp(&b)));
    let topk: Vec<usize> = order.into_iter().take(BEAMS_TO_KEEP).collect();
    let topk_scores: Vec<f64> = topk.iter().map(|&i| flat[i]).collect();
    let topk_beams: Vec<usize> = topk.iter().map(|&i| i / VOCAB_SIZE).collect();
    let topk_seqs: Vec<Vec<i64>> = topk
        .iter()
        .map(|&i| {
            let mut seq = running[i / VOCAB_SIZE].clone();
            seq.push((i % VOCAB_SIZE) as i64);
            seq
        })
        .collect();
    let hits_stop: Vec<bool> = topk_seqs
        .iter()
        .map(|seq| {
            *seq.last().expect("top-K seqs are never empty") == EOS_ID || seq.len() >= MAX_LENGTH
        })
        .collect();
    // Running beams for the next step: best non-finished top-K.
    let masked: Vec<f64> = topk_scores
        .iter()
        .enumerate()
        .map(|(i, &s)| if hits_stop[i] { s - 1e9 } else { s })
        .collect();
    let mut next_order: Vec<usize> = (0..BEAMS_TO_KEEP).collect();
    next_order.sort_by(|&a, &b| masked[b].total_cmp(&masked[a]).then(a.cmp(&b)));
    let next: Vec<usize> = next_order.into_iter().take(NUM_BEAMS).collect();
    let seqs: Vec<Vec<i64>> = next.iter().map(|&i| topk_seqs[i].clone()).collect();
    let scores: Vec<f64> = next.iter().map(|&i| masked[i]).collect();
    let beams: Vec<usize> = next.iter().map(|&i| topk_beams[i]).collect();
    // Finalize: finished beams among the top num_beams only, merged with
    // the previous finalized set, best kept. Ties keep the earlier entry,
    // as in the reference.
    // cur_len + 1 tokens, decoder prompt len 1.
    let gen_len = (cur_len as f64).powf(LENGTH_PENALTY);
    let all_done = finalized.iter().all(|entry| entry.2);
    let fresh: Vec<(Vec<i64>, f64, bool)> = (0..BEAMS_TO_KEEP)
        .map(|i| {
            let mut score = topk_scores[i] / gen_len;
            if all_done {
                score = -1e9;
            }
            if !heuristic_ok {
                score = -1e9;
            }
            let just_done = hits_stop[i] && i < NUM_BEAMS;
            if !just_done {
                score = -1e9;
            }
            (topk_seqs[i].clone(), score, just_done)
        })
        .collect();
    let mut merged: Vec<(Vec<i64>, f64, bool)> = finalized.to_vec();
    merged.extend(fresh);
    let mut best_order: Vec<usize> = (0..merged.len()).collect();
    best_order.sort_by(|&a, &b| merged[b].1.total_cmp(&merged[a].1).then(a.cmp(&b)));
    let done: Vec<(Vec<i64>, f64, bool)> = best_order
        .into_iter()
        .take(NUM_BEAMS)
        .map(|i| merged[i].clone())
        .collect();
    // Early-stop heuristic: the best running score, penalized at the
    // hypothetical best length, can still beat the worst finished beam.
    let hypo_len = (cur_len as f64).powf(LENGTH_PENALTY);
    let best_running = scores[0] / hypo_len;
    let mut worst_done = -1e9;
    let mut any_done = false;
    for entry in done.iter().filter(|entry| entry.2) {
        if !any_done || entry.1 < worst_done {
            worst_done = entry.1;
            any_done = true;
        }
    }
    let still_ok = heuristic_ok && best_running > worst_done;
    let open_beam = !done.iter().all(|entry| entry.2);
    let keep_going = still_ok && open_beam && !hits_stop.iter().all(|&hit| hit);
    BeamPick {
        seqs,
        scores,
        beams,
        finalized: done,
        heuristic_ok: still_ok,
        keep_going,
    }
}

/// Repeats one batch-1 [head, len, d] cache to all beams.
fn repeat_beams(single: &[f32]) -> Vec<f32> {
    let mut out = Vec::with_capacity(single.len() * NUM_BEAMS);
    for _ in 0..NUM_BEAMS {
        out.extend_from_slice(single);
    }
    out
}

/// Gathers one beam slice per selected beam from a grown [beam, head, len,
/// d] cache (HF `_reorder_cache`).
fn gather_beams(present: &[f32], beams: &[usize], len: usize) -> Vec<f32> {
    let beam_size = NUM_HEADS * len * D_KV;
    let mut out = vec![0.0; beams.len() * beam_size];
    for (b, &beam) in beams.iter().enumerate() {
        let from = beam * beam_size;
        out[b * beam_size..(b + 1) * beam_size].copy_from_slice(&present[from..from + beam_size]);
    }
    out
}

/// ByT5 byte ids: UTF-8 byte b → id b + 3.
fn encode_bytes(text: &str) -> Vec<i64> {
    text.as_bytes()
        .iter()
        .map(|&b| i64::from(b) + BYTE_OFFSET)
        .collect()
}

/// Relative-position buckets for one decode step: query position [past_len]
/// over keys 0..past_len, decoder-style (bidirectional = false). Exact port
/// of `T5Attention._relative_position_bucket` (and of `self_bias_buckets` in
/// `export_g2p.py`, which the export check proves against HF on every
/// reachable length).
fn self_bias_buckets(past_len: usize) -> Vec<i64> {
    let max_exact = NUM_BUCKETS / 2;
    (0..=past_len)
        .map(|key| {
            let rel = past_len - key;
            if rel < max_exact {
                rel as i64
            } else {
                let large = max_exact
                    + ((rel as f64 / max_exact as f64).ln()
                        / (MAX_DISTANCE as f64 / max_exact as f64).ln()
                        * (NUM_BUCKETS - max_exact) as f64) as usize;
                large.min(NUM_BUCKETS - 1) as i64
            }
        })
        .collect()
}

/// ByT5 decode: ids 3..258 → bytes, everything else skipped, then UTF-8
/// with malformed input ignored (reference `convert_tokens_to_string`).
fn decode_bytes(ids: &[i64]) -> String {
    let bytes: Vec<u8> = ids
        .iter()
        .filter(|&&id| (FIRST_BYTE_ID..=LAST_BYTE_ID).contains(&id))
        .map(|&id| (id - BYTE_OFFSET) as u8)
        .collect();
    let mut out = String::new();
    for chunk in bytes.utf8_chunks() {
        out.push_str(chunk.valid());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_codec_round_trips_and_skips_specials() {
        assert_eq!(encode_bytes("ab"), vec![100, 101]);
        assert_eq!(decode_bytes(&[100, 101]), "ab");
        // Multibyte UTF-8 survives the round trip.
        assert_eq!(decode_bytes(&encode_bytes("می")), "می");
        // Specials skipped, malformed bytes ignored.
        assert_eq!(decode_bytes(&[PAD_ID, EOS_ID, 259, 384]), "");
        assert_eq!(decode_bytes(&[0xd8 + BYTE_OFFSET]), "");
        assert_eq!(decode_bytes(&[100, EOS_ID, 101]), "ab");
    }

    #[test]
    fn bias_buckets_cover_short_and_long_distances() {
        assert_eq!(self_bias_buckets(0), vec![0]);
        assert_eq!(self_bias_buckets(1), vec![1, 0]);
        let buckets = self_bias_buckets(16);
        assert_eq!(buckets.len(), 17);
        assert_eq!((buckets[0], buckets[1], buckets[16]), (16, 15, 0));
        assert_eq!(self_bias_buckets(17)[0], 16);
        assert_eq!(self_bias_buckets(128)[0], 31);
        let buckets = self_bias_buckets(200);
        assert_eq!(buckets.len(), 201);
        assert!(buckets.windows(2).all(|pair| pair[0] >= pair[1]));
    }

    #[test]
    fn beam_caches_repeat_and_gather() {
        let repeated = repeat_beams(&[1.0, 2.0]);
        assert_eq!(repeated.len(), 2 * NUM_BEAMS);
        assert_eq!(&repeated[..2], &[1.0, 2.0]);
        assert_eq!(&repeated[2..4], &[1.0, 2.0]);
        // Gathering [1, 0] swaps the two beam slices.
        let beam_size = NUM_HEADS * D_KV;
        let mut present = vec![0.0; 2 * beam_size];
        present[0] = 10.0;
        present[beam_size] = 20.0;
        let gathered = gather_beams(&present, &[1, 0], 1);
        assert_eq!(gathered.len(), 2 * beam_size);
        assert_eq!(gathered[0], 20.0);
        assert_eq!(gathered[beam_size], 10.0);
    }

    #[test]
    fn beam_selection_finalizes_eos_and_stops_when_beaten() {
        // Degenerate all-zero logits: every beam-0 token ties, so the first
        // ten token ids win in order; token 1 is EOS.
        let logits = vec![0.0; NUM_BEAMS * VOCAB_SIZE];
        let running = vec![vec![PAD_ID]; NUM_BEAMS];
        let running_scores = vec![0.0, -1e9, -1e9, -1e9, -1e9];
        let finalized = vec![(vec![PAD_ID], -1e9, false); NUM_BEAMS];
        let pick = select_beams(&logits, &running, &running_scores, &finalized, true, 1);
        // Running beams skip the finished [0, 1].
        assert_eq!(
            pick.seqs,
            vec![vec![0, 0], vec![0, 2], vec![0, 3], vec![0, 4], vec![0, 5]]
        );
        assert_eq!(pick.beams, vec![0, 0, 0, 0, 0]);
        // The EOS beam finalizes with the uniform log-prob…
        assert_eq!(pick.finalized[0].0, vec![0, 1]);
        assert!(pick.finalized[0].2);
        assert!((pick.finalized[0].1 + (VOCAB_SIZE as f64).ln()).abs() < 1e-9);
        // …and nothing running can beat it, so the search stops.
        assert!(!pick.keep_going);
    }
}
