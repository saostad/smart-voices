//! Pocket TTS audio recipe: per-piece trim and speed, pauses by piece end,
//! piece finishing.

use super::mastering::master_chunk;
use super::text::PieceEnd;

/// Silence kept at each end when trimming a chunk (outer ends always keep
/// this).
const TRIM_KEEP_MS: u64 = 120;
/// Silence kept at the cut side of a clause boundary (`،`, `؛`, …).
const TRIM_KEEP_CLAUSE_MS: u64 = 60;
/// Silence kept at the cut side of a mid-sentence cut (no punctuation).
const TRIM_KEEP_MID_MS: u64 = 25;
/// Pause after a sentence-final piece; mid-sentence cuts get no pause.
const PAUSE_SENTENCE_MS: u64 = 150;
/// Pause after a clause-final piece.
const PAUSE_CLAUSE_MS: u64 = 60;
/// Samples at or below this magnitude count as silence.
const TRIM_THRESHOLD: f32 = 0.01;

/// Silence kept where a piece meets a boundary of `end` (outer ends keep
/// [`TRIM_KEEP_MS`]).
fn trim_keep_ms(end: PieceEnd) -> u64 {
    match end {
        PieceEnd::Sentence => TRIM_KEEP_MS,
        PieceEnd::Clause => TRIM_KEEP_CLAUSE_MS,
        PieceEnd::Mid => TRIM_KEEP_MID_MS,
    }
}

/// Pause after a piece ending with `end` (mid-sentence cuts get none).
fn pause_after_ms(end: PieceEnd) -> u64 {
    match end {
        PieceEnd::Sentence => PAUSE_SENTENCE_MS,
        PieceEnd::Clause => PAUSE_CLAUSE_MS,
        PieceEnd::Mid => 0,
    }
}

/// Trim leading/trailing silence (samples at or below the threshold) while
/// keeping `keep_start_ms` of it at the start and `keep_end_ms` at the end.
/// An all-silent input stays as-is shortened to the kept ends, so a silent
/// chunk still has its shape.
fn trim_silence(
    samples: &[f32],
    sample_rate: u32,
    keep_start_ms: u64,
    keep_end_ms: u64,
) -> Vec<f32> {
    let keep_start = (keep_start_ms as f64 / 1000.0 * f64::from(sample_rate)).round() as usize;
    let keep_end = (keep_end_ms as f64 / 1000.0 * f64::from(sample_rate)).round() as usize;
    let first = samples.iter().position(|s| s.abs() > TRIM_THRESHOLD);
    let last = samples.iter().rposition(|s| s.abs() > TRIM_THRESHOLD);
    let (Some(first), Some(last)) = (first, last) else {
        return samples
            .iter()
            .take(keep_start + keep_end)
            .copied()
            .collect();
    };
    let start = first.saturating_sub(keep_start);
    let end = (last + 1 + keep_end).min(samples.len());
    samples[start..end].to_vec()
}

/// Raw generated audio for one piece, plus how its end sounds (decided
/// from its text by the caller).
pub(crate) struct RawPiece {
    pub(crate) audio: Vec<f32>,
    pub(crate) end: PieceEnd,
}

/// Per-piece emission callback for [`finish_speech`]: each finished piece
/// and pause, in order.
type OnPiece<'a> = Option<&'a mut dyn FnMut(&[f32])>;

/// Finish raw pieces into playable audio: per piece, trim (the first
/// piece's start keeps [`TRIM_KEEP_MS`]; the last piece's end keeps
/// [`TRIM_KEEP_CLAUSE_MS`] for a clause end and [`TRIM_KEEP_MS`] otherwise;
/// cut sides keep [`trim_keep_ms`] of their boundary), then speed, then
/// [`master_chunk`]; between pieces, the boundary's pause
/// ([`pause_after_ms`], none mid-sentence). At a mid-sentence boundary the
/// kept silences are overlap-added instead of butt-joined: the previous
/// piece's last `n` samples fade out while the next piece's first `n`
/// fade in and the two are summed, where `n` is the smaller of the
/// [`TRIM_KEEP_MID_MS`] length after speed and the two piece lengths, so
/// the output is `n` samples shorter per mid boundary. The tail of a
/// mid-ended piece is held back until the next piece is ready, so
/// `on_piece` still receives exactly the audio that is returned, in
/// order. Each finished piece and pause also goes to `on_piece` as it is
/// ready, so streaming callers hear pieces one by one.
pub(crate) fn finish_speech(
    pieces: &[RawPiece],
    sample_rate: u32,
    speed: f32,
    mut on_piece: OnPiece<'_>,
) -> Vec<f32> {
    let pause_len = |ms: u64| (ms as f64 / 1000.0 * f64::from(sample_rate)) as usize;
    let mut out: Vec<f32> = Vec::new();
    let mut emit = |part: &[f32], out: &mut Vec<f32>| {
        if let Some(callback) = on_piece.as_deref_mut() {
            callback(part);
        }
        out.extend_from_slice(part);
    };
    let mut finished: Vec<Vec<f32>> = Vec::with_capacity(pieces.len());
    for (index, raw) in pieces.iter().enumerate() {
        let keep_start = if index == 0 {
            TRIM_KEEP_MS
        } else {
            trim_keep_ms(pieces[index - 1].end)
        };
        let keep_end = if index + 1 == pieces.len() {
            if raw.end == PieceEnd::Clause {
                TRIM_KEEP_CLAUSE_MS
            } else {
                TRIM_KEEP_MS
            }
        } else {
            trim_keep_ms(raw.end)
        };
        let piece = trim_silence(&raw.audio, sample_rate, keep_start, keep_end);
        let piece = apply_speed(&piece, speed);
        let piece = master_chunk(&piece, sample_rate);
        finished.push(piece);
    }
    let mid_keep = (TRIM_KEEP_MID_MS as f64 / 1000.0 * f64::from(sample_rate)).round() as usize;
    let mid_after_speed = if speed.is_finite() && speed > 0.0 {
        (mid_keep as f32 / speed).round() as usize
    } else {
        mid_keep
    };
    let overlaps: Vec<usize> = (0..pieces.len().saturating_sub(1))
        .map(|index| {
            if pieces[index].end == PieceEnd::Mid {
                mid_after_speed
                    .min(finished[index].len())
                    .min(finished[index + 1].len())
            } else {
                0
            }
        })
        .collect();
    let mut held_tail: Option<Vec<f32>> = None;
    for (index, piece) in finished.iter().enumerate() {
        let overlap_out = overlaps.get(index).copied().unwrap_or(0);
        let mut segment: Vec<f32> = Vec::new();
        let start = if let Some(tail) = held_tail.take() {
            let n = tail.len().min(piece.len());
            for (k, (prev, next)) in tail.iter().zip(piece.iter()).take(n).enumerate() {
                let fade_in = (k + 1) as f32 / (n + 1) as f32;
                segment.push(prev * (1.0 - fade_in) + next * fade_in);
            }
            n
        } else {
            0
        };
        let body_end = piece.len() - overlap_out;
        if body_end >= start {
            segment.extend_from_slice(&piece[start..body_end]);
        }
        if overlap_out > 0 {
            held_tail = Some(piece[body_end..].to_vec());
        }
        emit(&segment, &mut out);
        if index + 1 < pieces.len() {
            let pause_ms = pause_after_ms(pieces[index].end);
            if pause_ms > 0 {
                let pause = vec![0.0; pause_len(pause_ms)];
                emit(&pause, &mut out);
            }
        }
    }
    out
}

/// Resample by linear interpolation so playback lasts `1/speed` as long.
pub(crate) fn apply_speed(samples: &[f32], speed: f32) -> Vec<f32> {
    if !speed.is_finite() || (speed - 1.0).abs() < f32::EPSILON || samples.len() < 2 {
        return samples.to_vec();
    }
    let out_len = ((samples.len() as f32 / speed).round() as usize).max(1);
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let pos = i as f32 * speed;
        let index = pos.floor() as usize;
        let frac = pos - index as f32;
        let a = samples[index.min(samples.len() - 1)];
        let b = samples[(index + 1).min(samples.len() - 1)];
        out.push(a + (b - a) * frac);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trim_silence_keeps_each_end() {
        let sample_rate = 24_000u32;
        let keep = 2_880usize;
        let mut samples = vec![0.0; 24_000];
        samples[12_000] = 0.5;
        let trimmed = trim_silence(&samples, sample_rate, TRIM_KEEP_MS, TRIM_KEEP_MS);
        assert_eq!(trimmed.len(), keep * 2 + 1);
        assert_eq!(trimmed[keep], 0.5);
        // Loud edges are untouched.
        let loud = vec![0.5; 100];
        assert_eq!(
            trim_silence(&loud, sample_rate, TRIM_KEEP_MS, TRIM_KEEP_MS),
            loud
        );
        // Asymmetric keeps for cut sides.
        let trimmed = trim_silence(&samples, sample_rate, TRIM_KEEP_CLAUSE_MS, TRIM_KEEP_MID_MS);
        assert_eq!(trimmed.len(), 1_440 + 1 + 600);
    }

    #[test]
    fn pause_rule_keep_and_pause_lengths() {
        assert_eq!(trim_keep_ms(PieceEnd::Sentence), 120);
        assert_eq!(trim_keep_ms(PieceEnd::Clause), 60);
        assert_eq!(trim_keep_ms(PieceEnd::Mid), 25);
        assert_eq!(pause_after_ms(PieceEnd::Sentence), 150);
        assert_eq!(pause_after_ms(PieceEnd::Clause), 60);
        assert_eq!(pause_after_ms(PieceEnd::Mid), 0);
    }

    /// One loud blip in silence: trim keeps are exact either side of it.
    fn blip_piece(silence: usize, end: PieceEnd) -> RawPiece {
        let mut audio = vec![0.0; silence * 2 + 1];
        audio[silence] = 0.5;
        RawPiece { audio, end }
    }

    #[test]
    fn finish_speech_applies_pause_rules_per_boundary() {
        let sample_rate = 24_000u32;
        let pieces = [
            blip_piece(5_000, PieceEnd::Sentence),
            blip_piece(5_000, PieceEnd::Clause),
            blip_piece(5_000, PieceEnd::Mid),
        ];
        let mut emitted: Vec<Vec<f32>> = Vec::new();
        let mut emit = |part: &[f32]| emitted.push(part.to_vec());
        let audio = finish_speech(&pieces, sample_rate, 1.0, Some(&mut emit));
        // Trim keeps per boundary (mastering and speed 1.0 keep lengths):
        // first start 120 ms; sentence cut sides 120 ms; clause cut sides
        // 60 ms; the last piece ends Mid, so its outer end keeps 120 ms
        // and no overlap is subtracted (there is no next piece). Pauses:
        // 150 ms after the sentence piece, 60 ms after the clause piece,
        // none after the mid piece.
        let finished = [2_880 + 1 + 2_880, 2_880 + 1 + 1_440, 1_440 + 1 + 2_880];
        let pauses = [3_600, 1_440];
        let expected: usize = finished.iter().sum::<usize>() + pauses.iter().sum::<usize>();
        assert_eq!(audio.len(), expected);
        // The returned audio concatenates exactly what was emitted.
        let flat: Vec<f32> = emitted.concat();
        assert_eq!(audio, flat);
        assert_eq!(emitted.len(), 5);
        assert_eq!(emitted[0].len(), finished[0]);
        assert_eq!(emitted[2].len(), finished[1]);
        assert_eq!(emitted[4].len(), finished[2]);
        // Pauses are exact-length all-zero spans in place.
        let pause_at = [finished[0], finished[0] + pauses[0] + finished[1]];
        for (span, len) in pause_at.iter().zip(pauses) {
            assert!(audio[*span..*span + len].iter().all(|s| *s == 0.0));
        }
        // The blips survived mastering where the keeps put them.
        assert_ne!(audio[2_880], 0.0);
        assert_ne!(audio[finished[0] + pauses[0] + 2_880], 0.0);
    }

    #[test]
    fn finish_speech_mid_boundary_overlap_adds_without_a_jump() {
        let sample_rate = 24_000u32;
        let pieces = [
            RawPiece {
                audio: vec![0.5; 4_800],
                end: PieceEnd::Mid,
            },
            RawPiece {
                audio: vec![0.5; 4_800],
                end: PieceEnd::Sentence,
            },
        ];
        let mut emitted: Vec<Vec<f32>> = Vec::new();
        let mut emit = |part: &[f32]| emitted.push(part.to_vec());
        let audio = finish_speech(&pieces, sample_rate, 1.0, Some(&mut emit));
        // 25 ms (600 samples) shorter than the butt join, no pause.
        assert_eq!(audio.len(), 4_800 + 4_800 - 600);
        assert_eq!(audio, emitted.concat());
        // At the join (the overlapped 600 samples start at 4 200) no pair
        // jumps by more than the fade step and the join is not all zero.
        let join = 4_200;
        let fade_step = 1.0 / 601.0 + 0.01;
        for pair in audio[join - 1..join + 600].windows(2) {
            assert!(
                (pair[1] - pair[0]).abs() <= fade_step,
                "jump {} at the join",
                (pair[1] - pair[0]).abs()
            );
        }
        assert!(audio[join..join + 600].iter().any(|s| *s != 0.0));
        assert!(
            !audio[join..join + 600]
                .windows(50)
                .any(|run| run.iter().all(|s| *s == 0.0)),
            "all-zero run at the join"
        );
    }

    #[test]
    fn finish_speech_short_pieces_do_not_panic_at_mid_boundary() {
        let sample_rate = 24_000u32;
        let pieces = [
            RawPiece {
                audio: vec![0.5; 10],
                end: PieceEnd::Mid,
            },
            RawPiece {
                audio: vec![0.5; 10],
                end: PieceEnd::Sentence,
            },
        ];
        let audio = finish_speech(&pieces, sample_rate, 1.0, None);
        assert_eq!(audio.len(), 20 - 10);
    }

    #[test]
    fn finish_speech_single_clause_piece_keeps_60_ms_at_end() {
        let sample_rate = 24_000u32;
        let pieces = [blip_piece(5_000, PieceEnd::Clause)];
        let audio = finish_speech(&pieces, sample_rate, 1.0, None);
        // Start keeps 120 ms, the clause end keeps 60 ms.
        assert_eq!(audio.len(), 2_880 + 1 + 1_440);
    }

    #[test]
    fn speed_resample_changes_length_not_content_endpoints() {
        let samples: Vec<f32> = (0..100).map(|i| i as f32).collect();
        assert_eq!(apply_speed(&samples, 1.0).len(), 100);
        assert_eq!(apply_speed(&samples, 2.0).len(), 50);
        assert_eq!(apply_speed(&samples, 0.5).len(), 200);
    }
}
