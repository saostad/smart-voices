//! Pocket TTS (Farsi v1) engine on ONNX Runtime.
//!
//! The exported `.onnx` is one streaming step: text, voice conditioning, or
//! the previous latent goes in; one 80 ms audio frame plus the updated caches
//! come out. This module drives that graph the way the pocket-tts-onnx
//! runtimes do (`tts.py` / `tts.ts`), with the Farsi generation recipe from
//! the model's `farsi_tts.py` on top: 18-token chunks, runaway retry/split,
//! per-piece silence trim and mastering to about −10 LUFS, and pauses by
//! piece end (150 ms after sentences, 60 ms after clauses, none mid-sentence).
//!
//! Runtime sharing: sherpa-onnx is linked shared, so its `libonnxruntime`
//! sits next to the executable; `ort` (load-dynamic, API 28) loads that same
//! library — one ONNX Runtime in the process. If the library or the model
//! fails to load, the error says so; nothing falls back to Piper.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ort::session::{Session, SessionInputValue, SessionInputs};
use ort::value::Tensor;
use serde::Deserialize;

use crate::engine::EngineError;
use crate::normalize_fa::normalize;
use crate::sentencepiece::SentencePiece;

/// Directory under the models root holding the Pocket model.
pub const POCKET_DIR: &str = "pocket-farsi-v1";
/// The single exported model file inside [`POCKET_DIR`].
pub const POCKET_FILE: &str = "pocket-farsi-v1.onnx";

/// Farsi-verified generation settings (model card + blind-test harness).
const TEMPERATURE: f32 = 0.3;
const EOS_THRESHOLD: f32 = -2.0;
const FRAMES_AFTER_EOS: usize = 0;
const MAX_TOKENS_PER_CHUNK: usize = 18;
const SPLIT_SEAM: Duration = Duration::from_millis(50);
/// A generation longer than this share of its length cap never said EOS.
const RUNAWAY_RATIO: f64 = 0.97;
const MAX_SPLIT_DEPTH: usize = 2;
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
/// Mastered chunk target: about −10 LUFS after compression and limiting.
const TARGET_LUFS: f64 = -10.0;
/// Pre-compressor level: each chunk is gained here first (gain clamped
/// below).
const PRE_TARGET_LUFS: f64 = -20.0;
/// Pre-gain clamp in dB, so near-silent chunks can't explode.
const PRE_GAIN_MIN_DB: f64 = -12.0;
const PRE_GAIN_MAX_DB: f64 = 24.0;
/// Compressor: threshold, ratio, RMS detector ballistics.
const COMP_THRESHOLD_DB: f64 = -28.0;
const COMP_RATIO: f64 = 6.0;
const COMP_ATTACK_MS: f64 = 5.0;
const COMP_RELEASE_MS: f64 = 80.0;
/// Look-ahead limiter ceiling and ballistics; output never exceeds the
/// ceiling.
const PEAK_CEILING: f32 = 0.95;
const LIMITER_LOOKAHEAD_MS: f64 = 5.0;
const LIMITER_RELEASE_MS: f64 = 50.0;
/// Second-order Butterworth high-pass in front of the chain.
const HIGHPASS_FREQ_HZ: f64 = 70.0;

const TEXT_GATE: [f32; 3] = [1.0, 0.0, 0.0];
const LATENT_GATE: [f32; 3] = [0.0, 1.0, 0.0];
const COND_GATE: [f32; 3] = [0.0, 0.0, 1.0];

/// Model geometry and defaults baked into the ONNX metadata.
#[derive(Debug, Deserialize)]
struct PocketConfig {
    sample_rate: u32,
    frame_rate: f64,
    #[serde(default)]
    frame_size: usize,
    latent_dim: usize,
    model_dim: usize,
    flow_layers: usize,
    flow_heads: usize,
    flow_head_dim: usize,
    mimi_layers: usize,
    mimi_heads: usize,
    mimi_head_dim: usize,
    mimi_kv_len: usize,
    mimi_steps_per_latent: usize,
    conv_state_size: usize,
    sampler_decode_steps: f32,
    tokens_per_second_estimate: f64,
    gen_seconds_padding: f64,
    voices: std::collections::BTreeMap<String, Vec<usize>>,
}

impl PocketConfig {
    fn flow_stride(&self) -> usize {
        self.flow_layers * 2 * self.flow_heads * self.flow_head_dim
    }

    fn mimi_stride(&self) -> usize {
        self.mimi_layers * 2 * self.mimi_heads * self.mimi_head_dim
    }
}

/// Grow-once token-position cache, mirroring the reference `_Cache`.
struct Cache {
    buffer: Vec<f32>,
    stride: usize,
    length: usize,
}

impl Cache {
    fn new(stride: usize, capacity: usize) -> Self {
        Self {
            buffer: vec![0.0; capacity * stride],
            stride,
            length: 0,
        }
    }

    fn reserve(&mut self, extra: usize) {
        let needed = self.length + extra;
        let capacity = self.buffer.len() / self.stride;
        if needed > capacity {
            let mut grown = vec![0.0; needed.max(capacity * 2) * self.stride];
            grown[..self.length * self.stride]
                .copy_from_slice(&self.buffer[..self.length * self.stride]);
            self.buffer = grown;
        }
    }

    fn append(&mut self, values: &[f32]) {
        let frames = values.len() / self.stride;
        self.reserve(frames);
        let start = self.length * self.stride;
        self.buffer[start..start + values.len()].copy_from_slice(values);
        self.length += frames;
    }

    /// The full past, for the flow cache.
    fn window(&self) -> &[f32] {
        &self.buffer[..self.length * self.stride]
    }

    /// The last `size` entries, zero-padded at the front while still short.
    fn window_sized(&self, size: usize) -> Vec<f32> {
        if self.length >= size {
            let start = (self.length - size) * self.stride;
            return self.buffer[start..self.length * self.stride].to_vec();
        }
        let mut padded = vec![0.0; size * self.stride];
        let offset = (size - self.length) * self.stride;
        padded[offset..].copy_from_slice(&self.buffer[..self.length * self.stride]);
        padded
    }
}

/// Small self-contained Gaussian RNG (SplitMix64 + Box–Muller). Sampling must
/// not need a crates.io RNG, and a fresh stream per engine is fine.
struct Rng {
    state: u64,
    spare: Option<f32>,
}

impl Rng {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9e37_79b9_7f4a_7c15);
        Self {
            state: nanos ^ u64::from(std::process::id()).wrapping_mul(0x2545_f491_4f6c_dd1d),
            spare: None,
        }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn next_f32(&mut self) -> f32 {
        // 24-bit uniform in (0, 1].
        let value = (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32;
        if value <= 0.0 { 1.0 } else { value }
    }

    fn gaussian(&mut self) -> f32 {
        if let Some(spare) = self.spare.take() {
            return spare;
        }
        let u1 = self.next_f32();
        let u2 = self.next_f32();
        let radius = (-2.0 * u1.ln()).sqrt();
        let theta = 2.0 * std::f32::consts::PI * u2;
        self.spare = Some(radius * theta.sin());
        radius * theta.cos()
    }

    fn noise(&mut self, dim: usize, temperature: f32) -> Vec<f32> {
        let std = temperature.sqrt();
        (0..dim).map(|_| self.gaussian() * std).collect()
    }
}

/// IEEE-754 half → single conversion for the baked voice conditioning.
fn f16_to_f32(bits: u16) -> f32 {
    let sign = u32::from(bits >> 15) << 31;
    let exponent = i32::from((bits >> 10) & 0x1f);
    let fraction = u32::from(bits & 0x3ff);
    let out = match exponent {
        0 if fraction == 0 => sign,
        0 => {
            // Subnormal half: normalize.
            let mut e = -14i32;
            let mut f = fraction;
            while f & 0x400 == 0 {
                f <<= 1;
                e -= 1;
            }
            f &= 0x3ff;
            sign | (((e + 127) as u32) << 23) | (f << 13)
        }
        31 => sign | 0x7f80_0000 | (fraction << 13),
        e => sign | (((e - 15 + 127) as u32) << 23) | (fraction << 13),
    };
    f32::from_bits(out)
}

fn decode_f16_base64(text: &str) -> Result<Vec<f32>, EngineError> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(text)
        .map_err(|err| EngineError::PocketData(format!("voice data is not base64: {err}")))?;
    if bytes.len() % 2 != 0 {
        return Err(EngineError::PocketData(
            "voice data has an odd byte count".to_string(),
        ));
    }
    Ok(bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| f16_to_f32(u16::from_le_bytes([pair[0], pair[1]])))
        .collect())
}

/// The one ONNX Runtime environment for the process, loaded from the
/// `libonnxruntime` sherpa-onnx ships next to the executable.
fn ort_environment() -> Result<&'static ort::environment::Environment, EngineError> {
    static ENV: OnceLock<Result<ort::environment::Environment, String>> = OnceLock::new();
    ENV.get_or_init(|| {
        let candidates = ort_dylib_candidates();
        let mut last_error = String::new();
        for candidate in &candidates {
            match ort::init_from(candidate) {
                Ok(builder) => match builder.build() {
                    Ok(env) => return Ok(env),
                    Err(err) => last_error = format!("{}: {err}", candidate.display()),
                },
                Err(err) => last_error = format!("{}: {err}", candidate.display()),
            }
        }
        Err(format!(
            "no usable ONNX Runtime library (tried {}): {last_error}",
            candidates
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ))
    })
    .as_ref()
    .map_err(|message| EngineError::PocketRuntime(message.clone()))
}

fn ort_dylib_candidates() -> Vec<PathBuf> {
    let file = if cfg!(target_os = "windows") {
        "onnxruntime.dll"
    } else if cfg!(target_os = "macos") {
        "libonnxruntime.dylib"
    } else {
        "libonnxruntime.so"
    };
    let mut candidates = Vec::new();
    if let Ok(path) = std::env::var("ORT_DYLIB_PATH")
        && !path.is_empty()
    {
        candidates.push(PathBuf::from(path));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        candidates.push(dir.join(file));
        candidates.push(dir.join("lib").join(file));
    }
    candidates.push(PathBuf::from(file));
    candidates
}

fn thread_count() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, 8)
}

/// Prepare one chunk the way the reference runtime does before tokenizing:
/// flatten line breaks, uppercase the first character, and close a trailing
/// word character with a period. (The Farsi config disables space padding
/// and semicolon removal, and Persian has no case to change.)
fn prepare_text_prompt(chunk: &str) -> String {
    let mut text = chunk.trim().replace(['\n', '\r'], " ");
    while text.contains("  ") {
        text = text.replace("  ", " ");
    }
    if text.is_empty() {
        return text;
    }
    let mut chars = text.chars();
    if let Some(first) = chars.next() {
        let upper: String = first.to_uppercase().collect();
        text = format!("{upper}{}", chars.collect::<String>());
    }
    if text.chars().last().is_some_and(char::is_alphanumeric) {
        text.push('.');
    }
    text
}

const HARD_BREAKS: [char; 3] = ['.', '!', '؟'];
const SOFT_BREAKS: [char; 3] = ['،', '؛', ':'];

fn split_after(text: &str, breaks: &[char]) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < chars.len() {
        if breaks.contains(&chars[i]) && i + 1 < chars.len() && chars[i + 1].is_whitespace() {
            let piece: String = chars[start..=i].iter().collect();
            let piece = piece.trim().to_string();
            if !piece.is_empty() {
                parts.push(piece);
            }
            i += 1;
            while i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }
            start = i;
        } else {
            i += 1;
        }
    }
    if start < chars.len() {
        let piece: String = chars[start..].iter().collect();
        let piece = piece.trim().to_string();
        if !piece.is_empty() {
            parts.push(piece);
        }
    }
    if parts.len() > 1 { parts } else { Vec::new() }
}

/// Split normalized text into chunks of at most `max_tokens`, breaking at
/// sentence punctuation first, then clause punctuation, then words — a port
/// of `split_text` in the model's `farsi_tts.py` (with the default
/// `keep_punct_boundaries=false`, so neighbours merge back while they fit).
pub fn split_text(
    text: &str,
    count_tokens: &dyn Fn(&str) -> usize,
    max_tokens: usize,
) -> Vec<String> {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return Vec::new();
    }
    fn recurse(
        piece: &str,
        count_tokens: &dyn Fn(&str) -> usize,
        max_tokens: usize,
    ) -> Vec<String> {
        if count_tokens(piece) <= max_tokens {
            return vec![piece.to_string()];
        }
        for breaks in [HARD_BREAKS.as_slice(), SOFT_BREAKS.as_slice()] {
            let parts = split_after(piece, breaks);
            if !parts.is_empty() {
                return parts
                    .iter()
                    .flat_map(|part| recurse(part, count_tokens, max_tokens))
                    .collect();
            }
        }
        let mut out = Vec::new();
        let mut current = String::new();
        for word in piece.split_whitespace() {
            let trial = if current.is_empty() {
                word.to_string()
            } else {
                format!("{current} {word}")
            };
            if !current.is_empty() && count_tokens(&trial) > max_tokens {
                out.push(std::mem::take(&mut current));
                current = word.to_string();
            } else {
                current = trial;
            }
        }
        if !current.is_empty() {
            out.push(current);
        }
        out
    }
    let parts = recurse(&text, count_tokens, max_tokens);
    let mut merged: Vec<String> = Vec::new();
    for chunk in parts {
        if let Some(last) = merged.last_mut() {
            let trial = format!("{last} {chunk}");
            if count_tokens(&trial) <= max_tokens {
                *last = trial;
                continue;
            }
        }
        merged.push(chunk);
    }
    merged
}

/// How the boundary after a piece sounds: a full stop keeps the old 120 ms
/// keeps and 150 ms pause, a clause mark keeps 60 ms and pauses 60 ms, and
/// a mid-sentence cut keeps 25 ms at each cut side with no pause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PieceEnd {
    Sentence,
    Clause,
    Mid,
}

/// Classify a piece's end by its last character: sentence punctuation (`.`,
/// `!`, `؟`, `?`), clause marks (`،`, `؛`, `:`, `,`, `;`), or a mid-sentence
/// cut (anything else, including empty text).
pub fn classify_piece_end(text: &str) -> PieceEnd {
    match text.chars().last() {
        Some('.') | Some('!') | Some('؟') | Some('?') => PieceEnd::Sentence,
        Some('،') | Some('؛') | Some(':') | Some(',') | Some(';') => PieceEnd::Clause,
        _ => PieceEnd::Mid,
    }
}

/// Silence kept where a piece meets a boundary of `end` (outer ends keep
/// [`TRIM_KEEP_MS`]).
pub fn trim_keep_ms(end: PieceEnd) -> u64 {
    match end {
        PieceEnd::Sentence => TRIM_KEEP_MS,
        PieceEnd::Clause => TRIM_KEEP_CLAUSE_MS,
        PieceEnd::Mid => TRIM_KEEP_MID_MS,
    }
}

/// Pause after a piece ending with `end` (mid-sentence cuts get none).
pub fn pause_after_ms(end: PieceEnd) -> u64 {
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
pub fn trim_silence(
    samples: &[f32],
    sample_rate: u32,
    keep_start_ms: u64,
    keep_end_ms: u64,
) -> Vec<f32> {
    let keep_start =
        (keep_start_ms as f64 / 1000.0 * f64::from(sample_rate)).round() as usize;
    let keep_end = (keep_end_ms as f64 / 1000.0 * f64::from(sample_rate)).round() as usize;
    let first = samples.iter().position(|s| s.abs() > TRIM_THRESHOLD);
    let last = samples.iter().rposition(|s| s.abs() > TRIM_THRESHOLD);
    let (Some(first), Some(last)) = (first, last) else {
        return samples.iter().take(keep_start + keep_end).copied().collect();
    };
    let start = first.saturating_sub(keep_start);
    let end = (last + 1 + keep_end).min(samples.len());
    samples[start..end].to_vec()
}

/// Raw generated audio for one piece, plus how its end sounds (decided
/// from its text by the caller).
pub struct RawPiece {
    pub audio: Vec<f32>,
    pub end: PieceEnd,
}

/// Per-piece emission callback for [`finish_speech`]: each finished piece
/// and pause, in order.
type OnPiece<'a> = Option<&'a mut dyn FnMut(&[f32])>;

/// Finish raw pieces into playable audio: per piece, trim (the first
/// piece's start and the last piece's end keep [`TRIM_KEEP_MS`]; cut sides
/// keep [`trim_keep_ms`] of their boundary), then speed, then
/// [`master_chunk`]; between pieces, the boundary's pause
/// ([`pause_after_ms`], none mid-sentence). Each finished piece and pause
/// also goes to `on_piece` as it is ready, so streaming callers hear pieces
/// one by one; the returned audio concatenates exactly what was emitted.
pub fn finish_speech(
    pieces: &[RawPiece],
    sample_rate: u32,
    speed: f32,
    mut on_piece: OnPiece<'_>,
) -> Vec<f32> {
    let pause_len = |ms: u64| (ms as f64 / 1000.0 * f64::from(sample_rate)) as usize;
    let sentence_pause = vec![0.0; pause_len(PAUSE_SENTENCE_MS)];
    let clause_pause = vec![0.0; pause_len(PAUSE_CLAUSE_MS)];
    let mut out: Vec<f32> = Vec::new();
    let mut emit = |part: &[f32], out: &mut Vec<f32>| {
        if let Some(callback) = on_piece.as_deref_mut() {
            callback(part);
        }
        out.extend_from_slice(part);
    };
    for (index, raw) in pieces.iter().enumerate() {
        let keep_start = if index == 0 {
            TRIM_KEEP_MS
        } else {
            trim_keep_ms(pieces[index - 1].end)
        };
        let keep_end = if index + 1 == pieces.len() {
            TRIM_KEEP_MS
        } else {
            trim_keep_ms(raw.end)
        };
        let piece = trim_silence(&raw.audio, sample_rate, keep_start, keep_end);
        let piece = apply_speed(&piece, speed);
        let piece = master_chunk(&piece, sample_rate);
        emit(&piece, &mut out);
        if index + 1 < pieces.len() {
            let pause = match raw.end {
                PieceEnd::Sentence => Some(sentence_pause.as_slice()),
                PieceEnd::Clause => Some(clause_pause.as_slice()),
                PieceEnd::Mid => None,
            };
            if let Some(pause) = pause {
                emit(pause, &mut out);
            }
        }
    }
    out
}

/// Resample by linear interpolation so playback lasts `1/speed` as long.
fn apply_speed(samples: &[f32], speed: f32) -> Vec<f32> {
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

/// ITU-R BS.1770-4 integrated loudness in LUFS: K-weighting (high-shelf
/// pre-filter + RLB high-pass, coefficients computed for `sample_rate` with
/// the libebur128/pyloudnorm formulas), 400 ms blocks with 75 % overlap,
/// −70 LUFS absolute gate and −10 LU relative gate. Shorter than one 400 ms
/// block: one ungated block over the whole input. All-silent input returns
/// negative infinity.
pub fn measure_lufs(samples: &[f32], sample_rate: u32) -> f64 {
    if samples.is_empty() || sample_rate == 0 {
        return f64::NEG_INFINITY;
    }
    let input: Vec<f64> = samples.iter().map(|s| f64::from(*s)).collect();
    let weighted = apply_biquad(&input, &k_shelf_coeffs(sample_rate));
    let weighted = apply_biquad(&weighted, &k_high_pass_coeffs(sample_rate));
    let block_size = sample_rate as usize * 400 / 1000;
    let hop_size = sample_rate as usize * 100 / 1000;
    if block_size == 0 || hop_size == 0 {
        return f64::NEG_INFINITY;
    }
    if weighted.len() < block_size {
        let mean_square = mean_square(&weighted, 0, weighted.len());
        if mean_square <= 0.0 {
            return f64::NEG_INFINITY;
        }
        return -0.691 + 10.0 * mean_square.log10();
    }
    let mut blocks = Vec::new();
    let mut start = 0;
    while start + block_size <= weighted.len() {
        blocks.push(mean_square(&weighted, start, start + block_size));
        start += hop_size;
    }
    if blocks.is_empty() {
        return f64::NEG_INFINITY;
    }
    let abs_gate = 10f64.powf((-70.0 + 0.691) / 10.0);
    let gated: Vec<f64> = blocks.into_iter().filter(|b| *b > abs_gate).collect();
    if gated.is_empty() {
        return f64::NEG_INFINITY;
    }
    let relative_loudness =
        -0.691 + 10.0 * (gated.iter().sum::<f64>() / gated.len() as f64).log10();
    let rel_gate = 10f64.powf((relative_loudness - 10.0 + 0.691) / 10.0);
    let kept: Vec<f64> = gated.into_iter().filter(|b| *b > rel_gate).collect();
    if kept.is_empty() {
        return f64::NEG_INFINITY;
    }
    -0.691 + 10.0 * (kept.iter().sum::<f64>() / kept.len() as f64).log10()
}

/// Master one chunk to about [`TARGET_LUFS`]: 70 Hz high-pass, pre-gain to
/// [`PRE_TARGET_LUFS`] (clamped), 6:1 compression, make-up gain to
/// [`TARGET_LUFS`], and a look-ahead peak limiter at [`PEAK_CEILING`]. The
/// limiter shaves loudness off the peaks it catches, so the output is
/// measured once more after limiting and the deficit is topped up at 4× as
/// headroom — hotter peaks catch the final limit harder, so only part
/// survives — before that final limit; the limiter still runs last, so no
/// output sample exceeds [`PEAK_CEILING`]. Output length equals input
/// length. All-zero input passes through unchanged.
pub fn master_chunk(samples: &[f32], sample_rate: u32) -> Vec<f32> {
    if samples.is_empty() {
        return Vec::new();
    }
    if samples.iter().all(|s| *s == 0.0) {
        return samples.to_vec();
    }
    let mut out = apply_high_pass(samples, sample_rate);
    let pre_loudness = measure_lufs(&out, sample_rate);
    if pre_loudness == f64::NEG_INFINITY {
        return samples.to_vec();
    }
    out = apply_gain_db(
        &out,
        (PRE_TARGET_LUFS - pre_loudness).clamp(PRE_GAIN_MIN_DB, PRE_GAIN_MAX_DB),
    );
    out = apply_compressor(&out, sample_rate);
    let post_loudness = measure_lufs(&out, sample_rate);
    if post_loudness == f64::NEG_INFINITY {
        return samples.to_vec();
    }
    out = apply_gain_db(&out, TARGET_LUFS - post_loudness);
    // Limiting only ever lowers loudness; without this the −10 target
    // measured −11.8 on the fixture (the limiter caught 1.8 LU of peaks).
    // The top-up quadruples the deficit as headroom: hotter peaks catch the
    // final limit harder, so only part of the top-up survives on real
    // speech.
    out = apply_limiter(&out, sample_rate);
    let limited = measure_lufs(&out, sample_rate);
    if limited.is_finite() && limited < TARGET_LUFS - 0.1 {
        out = apply_limiter(
            &apply_gain_db(&out, (TARGET_LUFS - limited) * 4.0),
            sample_rate,
        );
    }
    out
}

/// Second-order direct-form biquad: y = b0·x + b1·x1 + b2·x2 − a1·y1 − a2·y2.
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

/// K-weighting high-shelf pre-filter (libebur128/pyloudnorm formula for
/// `sample_rate`).
fn k_shelf_coeffs(sample_rate: u32) -> Biquad {
    let f0 = 1_681.974_450_955_533;
    let gain = 3.737_636_852_614_458;
    let q = 0.707_175_236_955_419_6;
    let k = (std::f64::consts::PI * f0 / f64::from(sample_rate)).tan();
    let vh = 10f64.powf(gain / 20.0);
    let vb = vh.powf(0.499_666_774_154_541_6);
    let a0 = 1.0 + k / q + k * k;
    Biquad {
        b0: (vh + vb * k / q + k * k) / a0,
        b1: 2.0 * (k * k - vh) / a0,
        b2: (vh - vb * k / q + k * k) / a0,
        a1: 2.0 * (k * k - 1.0) / a0,
        a2: (1.0 - k / q + k * k) / a0,
    }
}

/// K-weighting RLB high-pass (libebur128/pyloudnorm formula for
/// `sample_rate`).
fn k_high_pass_coeffs(sample_rate: u32) -> Biquad {
    high_pass_coeffs(38.135_470_876_024_44, 0.500_327_037_323_877_3, sample_rate)
}

/// Second-order high-pass with cutoff `f0` Hz and Q `q` at `sample_rate`.
fn high_pass_coeffs(f0: f64, q: f64, sample_rate: u32) -> Biquad {
    let k = (std::f64::consts::PI * f0 / f64::from(sample_rate)).tan();
    let a0 = 1.0 + k / q + k * k;
    Biquad {
        b0: 1.0 / a0,
        b1: -2.0 / a0,
        b2: 1.0 / a0,
        a1: 2.0 * (k * k - 1.0) / a0,
        a2: (1.0 - k / q + k * k) / a0,
    }
}

fn apply_biquad(input: &[f64], c: &Biquad) -> Vec<f64> {
    let mut out = vec![0.0; input.len()];
    let mut x1 = 0.0;
    let mut x2 = 0.0;
    let mut y1 = 0.0;
    let mut y2 = 0.0;
    for (n, x0) in input.iter().enumerate() {
        let y0 = c.b0 * x0 + c.b1 * x1 + c.b2 * x2 - c.a1 * y1 - c.a2 * y2;
        out[n] = y0;
        x2 = x1;
        x1 = *x0;
        y2 = y1;
        y1 = y0;
    }
    out
}

fn apply_high_pass(samples: &[f32], sample_rate: u32) -> Vec<f32> {
    // Second-order Butterworth: Q = 1/sqrt(2).
    let c = high_pass_coeffs(HIGHPASS_FREQ_HZ, 0.707_106_781_186_547_5, sample_rate);
    let mut out = vec![0.0; samples.len()];
    let mut x1 = 0.0;
    let mut x2 = 0.0;
    let mut y1 = 0.0;
    let mut y2 = 0.0;
    for (n, sample) in samples.iter().enumerate() {
        let x0 = f64::from(*sample);
        let y0 = c.b0 * x0 + c.b1 * x1 + c.b2 * x2 - c.a1 * y1 - c.a2 * y2;
        out[n] = y0 as f32;
        x2 = x1;
        x1 = x0;
        y2 = y1;
        y1 = y0;
    }
    out
}

fn apply_gain_db(samples: &[f32], gain_db: f64) -> Vec<f32> {
    if gain_db == 0.0 {
        return samples.to_vec();
    }
    let gain = 10f64.powf(gain_db / 20.0) as f32;
    samples.iter().map(|s| s * gain).collect()
}

/// Downward compressor with an RMS detector: the squared signal is smoothed
/// with the attack/release ballistics, then the static 6:1 curve above the
/// threshold is applied. The threshold test runs in the power domain, so
/// below-threshold samples skip the sqrt/pow entirely; above it the gain is
/// one pow, with no log.
fn apply_compressor(samples: &[f32], sample_rate: u32) -> Vec<f32> {
    let attack = 1.0 - (-1.0 / (COMP_ATTACK_MS / 1000.0 * f64::from(sample_rate))).exp();
    let release = 1.0 - (-1.0 / (COMP_RELEASE_MS / 1000.0 * f64::from(sample_rate))).exp();
    let thresh_power = 10f64.powf(COMP_THRESHOLD_DB / 10.0);
    let thresh_lin = 10f64.powf(COMP_THRESHOLD_DB / 20.0);
    // gainDb = k·(thr − detDb), so gain = (thrLin/detector)^k.
    let curve = 1.0 - 1.0 / COMP_RATIO;
    let mut out = vec![0.0; samples.len()];
    let mut envelope = 0.0;
    for (n, sample) in samples.iter().enumerate() {
        let x = f64::from(*sample);
        let squared = x * x;
        envelope += (if squared > envelope { attack } else { release }) * (squared - envelope);
        if envelope < thresh_power {
            out[n] = *sample;
            continue;
        }
        out[n] = (x * (thresh_lin / envelope.sqrt()).powf(curve)) as f32;
    }
    out
}

/// Look-ahead peak limiter: each sample's gain sees the peak
/// [`LIMITER_LOOKAHEAD_MS`] ahead, attacks instantly and releases over
/// [`LIMITER_RELEASE_MS`]. Offline, so the look-ahead delay is compensated
/// (no extra samples); the final clamp guarantees the ceiling against float
/// error. The look-ahead peak is a sliding-window maximum over
/// [n, n + look-ahead] with a monotonic deque, O(n).
fn apply_limiter(samples: &[f32], sample_rate: u32) -> Vec<f32> {
    if samples.is_empty() {
        return Vec::new();
    }
    let look_ahead =
        ((LIMITER_LOOKAHEAD_MS / 1000.0 * f64::from(sample_rate)).round() as usize).max(1);
    let release = 1.0 - (-1.0 / (LIMITER_RELEASE_MS / 1000.0 * f64::from(sample_rate))).exp();
    let mut targets = vec![0.0f32; samples.len()];
    // Indices with decreasing |sample|; each sample enters and leaves once.
    let mut deque = vec![0usize; samples.len()];
    let mut head = 0usize;
    let mut tail = 0usize;
    let first_end = samples.len().min(look_ahead + 1);
    for i in 0..first_end {
        while tail > head && samples[deque[tail - 1]].abs() <= samples[i].abs() {
            tail -= 1;
        }
        deque[tail] = i;
        tail += 1;
    }
    for (n, target) in targets.iter_mut().enumerate() {
        let peak = samples[deque[head]].abs();
        *target = if peak < 1e-10 {
            1.0
        } else {
            1.0f32.min(PEAK_CEILING / peak)
        };
        if deque[head] == n {
            head += 1;
        }
        let incoming = n + look_ahead + 1;
        if incoming < samples.len() {
            while tail > head && samples[deque[tail - 1]].abs() <= samples[incoming].abs() {
                tail -= 1;
            }
            deque[tail] = incoming;
            tail += 1;
        }
    }
    let mut out = vec![0.0f32; samples.len()];
    let mut smooth = 1.0f64;
    for (n, target) in targets.iter().enumerate() {
        let target = f64::from(*target);
        smooth = if target < smooth {
            target
        } else {
            smooth + (target - smooth) * release
        };
        let y = (f64::from(samples[n]) * smooth) as f32;
        out[n] = y.clamp(-PEAK_CEILING, PEAK_CEILING);
    }
    out
}

fn mean_square(samples: &[f64], from: usize, to: usize) -> f64 {
    let mut sum = 0.0;
    for sample in &samples[from..to] {
        sum += sample * sample;
    }
    sum / (to - from) as f64
}

/// Loaded Pocket TTS (Farsi v1) engine. One baked voice, 24 kHz mono out.
pub struct PocketEngine {
    session: Session,
    config: PocketConfig,
    tokenizer: SentencePiece,
    /// Flow-LM cache rows for the voice prompt, reused by every chunk.
    voice_flow: Vec<f32>,
    voice_len: usize,
    rng: Rng,
}

struct StepOutput {
    audio: Vec<f32>,
    next_latent: Vec<f32>,
    eos_logit: f32,
    flow_kv_new: Vec<f32>,
    mimi_kv_new: Vec<f32>,
    mimi_offset_out: i64,
    mimi_conv_out: Vec<f32>,
}

fn tensor_f32(shape: Vec<usize>, data: Vec<f32>) -> Result<Tensor<f32>, EngineError> {
    Tensor::from_array((shape, data.into_boxed_slice()))
        .map_err(|err| EngineError::PocketData(format!("couldn't build a model input: {err}")))
}

fn tensor_i64(shape: Vec<usize>, data: Vec<i64>) -> Result<Tensor<i64>, EngineError> {
    Tensor::from_array((shape, data.into_boxed_slice()))
        .map_err(|err| EngineError::PocketData(format!("couldn't build a model input: {err}")))
}

fn extract_f32(
    outputs: &ort::session::SessionOutputs<'_>,
    name: &str,
) -> Result<Vec<f32>, EngineError> {
    let value = outputs
        .get(name)
        .ok_or_else(|| EngineError::PocketData(format!("model gave no '{name}' output")))?;
    let (_, data) = value
        .try_extract_tensor::<f32>()
        .map_err(|err| EngineError::PocketData(format!("model output '{name}': {err}")))?;
    Ok(data.to_vec())
}

impl PocketEngine {
    /// Load the Pocket model from `models_root/pocket-farsi-v1/`.
    pub fn load(models_root: &Path) -> Result<Self, EngineError> {
        let dir = models_root.join(POCKET_DIR);
        let file = dir.join(POCKET_FILE);
        if !file.exists() {
            return Err(EngineError::MissingModel(file));
        }
        let env = ort_environment()?;
        let session = Session::builder(env)
            .map_err(|err| EngineError::PocketLoadFailed(dir.clone(), err.to_string()))?
            .with_intra_threads(thread_count())
            .map_err(|err| EngineError::PocketLoadFailed(dir.clone(), err.to_string()))?
            .commit_from_file(&file)
            .map_err(|err| EngineError::PocketLoadFailed(dir.clone(), err.to_string()))?;

        let (config, tokenizer, voice_cond, voice_shape) = {
            let metadata = session
                .metadata()
                .map_err(|err| EngineError::PocketLoadFailed(dir.clone(), err.to_string()))?;
            let config_text = metadata.custom("pocket_tts_config").ok_or_else(|| {
                EngineError::PocketLoadFailed(
                    dir.clone(),
                    "model has no pocket_tts_config metadata".to_string(),
                )
            })?;
            let config: PocketConfig = serde_json::from_str(&config_text).map_err(|err| {
                EngineError::PocketLoadFailed(
                    dir.clone(),
                    format!("model config is unreadable: {err}"),
                )
            })?;
            if config.sample_rate == 0
                || config.latent_dim == 0
                || config.model_dim == 0
                || config.frame_size == 0
            {
                return Err(EngineError::PocketLoadFailed(
                    dir.clone(),
                    "model config has a zero sample rate or dimension".to_string(),
                ));
            }
            let tokenizer_b64 = metadata.custom("pocket_tts_tokenizer").ok_or_else(|| {
                EngineError::PocketLoadFailed(
                    dir.clone(),
                    "model has no pocket_tts_tokenizer metadata".to_string(),
                )
            })?;
            let tokenizer_bytes = {
                use base64::Engine as _;
                base64::engine::general_purpose::STANDARD
                    .decode(&tokenizer_b64)
                    .map_err(|err| {
                        EngineError::PocketLoadFailed(
                            dir.clone(),
                            format!("model tokenizer is not base64: {err}"),
                        )
                    })?
            };
            let tokenizer = SentencePiece::from_bytes(&tokenizer_bytes)
                .map_err(|err| EngineError::PocketLoadFailed(dir.clone(), err.to_string()))?;
            let (voice_name, voice_shape) = config
                .voices
                .iter()
                .next()
                .map(|(n, s)| (n.clone(), s.clone()))
                .ok_or_else(|| {
                    EngineError::PocketLoadFailed(
                        dir.clone(),
                        "model carries no baked voice".to_string(),
                    )
                })?;
            let voice_b64 = metadata
                .custom(&format!("pocket_tts_voice/{voice_name}"))
                .ok_or_else(|| {
                    EngineError::PocketLoadFailed(
                        dir.clone(),
                        format!("model voice '{voice_name}' data is missing"),
                    )
                })?;
            let voice_cond = decode_f16_base64(&voice_b64)?;
            let expected: usize = voice_shape.iter().product();
            if voice_cond.len() != expected {
                return Err(EngineError::PocketLoadFailed(
                    dir.clone(),
                    format!(
                        "model voice has {} values, config says {expected}",
                        voice_cond.len()
                    ),
                ));
            }
            (config, tokenizer, voice_cond, voice_shape)
        };
        if voice_shape.len() != 2 || voice_shape[1] != config.model_dim {
            return Err(EngineError::PocketLoadFailed(
                dir.clone(),
                format!("model voice shape {voice_shape:?} is not [frames, model_dim]"),
            ));
        }

        let mut engine = Self {
            session,
            config,
            tokenizer,
            voice_flow: Vec::new(),
            voice_len: 0,
            rng: Rng::new(),
        };
        // Prefill the voice once; every chunk restarts from this cache.
        let voice_len = voice_shape[0];
        let flow = {
            let mut flow_cache = Cache::new(engine.config.flow_stride(), voice_len + 1);
            let mut mimi_cache = Cache::new(engine.config.mimi_stride(), engine.config.mimi_kv_len);
            let out = engine.run_step(
                &voice_cond,
                voice_len,
                COND_GATE,
                &[],
                &mut flow_cache,
                &mut mimi_cache,
                0,
                &vec![0.0; engine.config.conv_state_size],
                None,
            )?;
            out.flow_kv_new
        };
        engine.voice_flow = flow;
        engine.voice_len = voice_len;
        Ok(engine)
    }

    /// Output sample rate in Hz (24000 for the Farsi model).
    pub fn sample_rate(&self) -> i32 {
        self.config.sample_rate as i32
    }

    /// Run one streaming step. `seq_data` is the tokens for a text step, the
    /// conditioning for a voice step, and empty otherwise; the gate selects
    /// which. Mirrors `_run` in the reference runtime.
    #[allow(clippy::too_many_arguments)]
    fn run_step(
        &mut self,
        seq_data: &[f32],
        seq: usize,
        gates: [f32; 3],
        tokens: &[i64],
        flow_cache: &mut Cache,
        mimi_cache: &mut Cache,
        mimi_offset: i64,
        mimi_conv: &[f32],
        latent_and_bos: Option<(&[f32], f32)>,
    ) -> Result<StepOutput, EngineError> {
        let cfg = &self.config;
        let zero_latent = vec![0.0; seq * cfg.latent_dim];
        let zero_bos = vec![0.0; seq];
        let (latent, is_bos): (Vec<f32>, Vec<f32>) = match latent_and_bos {
            Some((latent, bos)) => (latent.to_vec(), vec![bos; seq]),
            None => (zero_latent, zero_bos),
        };
        let cond: Vec<f32> = if gates == COND_GATE {
            seq_data.to_vec()
        } else {
            vec![0.0; seq * cfg.model_dim]
        };
        let token_data: Vec<i64> = if gates == TEXT_GATE {
            tokens.to_vec()
        } else {
            vec![0; seq]
        };
        let noise = vec![0.0; cfg.latent_dim];
        let flow_window = flow_cache.window().to_vec();
        let flow_past = flow_cache.length;
        let mimi_window = mimi_cache.window_sized(cfg.mimi_kv_len);

        let inputs: Vec<(Cow<'_, str>, SessionInputValue<'_>)> = vec![
            (
                Cow::Borrowed("tokens"),
                tensor_i64(vec![1, seq], token_data)?.into(),
            ),
            (
                Cow::Borrowed("latent"),
                tensor_f32(vec![1, seq, cfg.latent_dim], latent)?.into(),
            ),
            (
                Cow::Borrowed("is_bos"),
                tensor_f32(vec![1, seq, 1], is_bos)?.into(),
            ),
            (
                Cow::Borrowed("cond"),
                tensor_f32(vec![1, seq, cfg.model_dim], cond)?.into(),
            ),
            (
                Cow::Borrowed("gates"),
                tensor_f32(vec![3], gates.to_vec())?.into(),
            ),
            (
                Cow::Borrowed("noise"),
                tensor_f32(vec![1, cfg.latent_dim], noise)?.into(),
            ),
            (
                Cow::Borrowed("flow_kv"),
                tensor_f32(
                    vec![
                        flow_past,
                        cfg.flow_layers,
                        2,
                        1,
                        cfg.flow_heads,
                        cfg.flow_head_dim,
                    ],
                    flow_window,
                )?
                .into(),
            ),
            (
                Cow::Borrowed("flow_offset"),
                tensor_i64(vec![], vec![flow_past as i64])?.into(),
            ),
            (
                Cow::Borrowed("mimi_kv"),
                tensor_f32(
                    vec![
                        cfg.mimi_kv_len,
                        cfg.mimi_layers,
                        2,
                        1,
                        cfg.mimi_heads,
                        cfg.mimi_head_dim,
                    ],
                    mimi_window,
                )?
                .into(),
            ),
            (
                Cow::Borrowed("mimi_offset"),
                tensor_i64(vec![], vec![mimi_offset])?.into(),
            ),
            (
                Cow::Borrowed("mimi_conv"),
                tensor_f32(vec![cfg.conv_state_size], mimi_conv.to_vec())?.into(),
            ),
            (
                Cow::Borrowed("decode_steps"),
                tensor_f32(vec![], vec![cfg.sampler_decode_steps])?.into(),
            ),
        ];
        let outputs = self
            .session
            .run(SessionInputs::<'_, '_, 0>::ValueMap(inputs))
            .map_err(|err| EngineError::PocketData(format!("model step failed: {err}")))?;
        let eos_values = extract_f32(&outputs, "eos_logit")?;
        Ok(StepOutput {
            audio: extract_f32(&outputs, "audio")?,
            next_latent: extract_f32(&outputs, "next_latent")?,
            eos_logit: eos_values.first().copied().unwrap_or(f32::NEG_INFINITY),
            flow_kv_new: extract_f32(&outputs, "flow_kv_new")?,
            mimi_kv_new: extract_f32(&outputs, "mimi_kv_new")?,
            mimi_offset_out: {
                let value = outputs.get("mimi_offset_out").ok_or_else(|| {
                    EngineError::PocketData("model gave no 'mimi_offset_out' output".to_string())
                })?;
                let (_, data) = value.try_extract_tensor::<i64>().map_err(|err| {
                    EngineError::PocketData(format!("model output 'mimi_offset_out': {err}"))
                })?;
                data.first().copied().unwrap_or(0)
            },
            mimi_conv_out: extract_f32(&outputs, "mimi_conv_out")?,
        })
    }

    /// Generate one chunk in a single attempt (no runaway recovery).
    fn generate_once(&mut self, chunk: &str) -> Result<Vec<f32>, EngineError> {
        let prepared = prepare_text_prompt(chunk);
        let tokens = self.tokenizer.encode(&prepared);
        if tokens.is_empty() {
            return Ok(Vec::new());
        }
        let max_frames = ((tokens.len() as f64 / self.config.tokens_per_second_estimate
            + self.config.gen_seconds_padding)
            * self.config.frame_rate)
            .ceil() as usize;

        let mut flow_cache = Cache::new(self.config.flow_stride(), self.voice_len + 256);
        flow_cache.append(&self.voice_flow.clone());
        let mut mimi_cache = Cache::new(
            self.config.mimi_stride(),
            self.config.mimi_kv_len + max_frames * self.config.mimi_steps_per_latent,
        );
        let mut mimi_conv = vec![0.0; self.config.conv_state_size];
        let mut mimi_offset: i64 = 0;

        // Text prefill: keep the flow cache, drop the audio.
        let text_out = self.run_step(
            &[],
            tokens.len(),
            TEXT_GATE,
            &tokens,
            &mut flow_cache,
            &mut mimi_cache,
            mimi_offset,
            &mimi_conv,
            None,
        )?;
        flow_cache.append(&text_out.flow_kv_new);

        let mut audio: Vec<f32> = Vec::new();
        let mut latent = vec![0.0; self.config.latent_dim];
        let mut eos_frame: Option<usize> = None;
        for frame in 0..max_frames {
            let out = self.run_step_with_noise(
                &latent,
                if frame == 0 { 1.0 } else { 0.0 },
                &mut flow_cache,
                &mut mimi_cache,
                mimi_offset,
                &mimi_conv,
            )?;
            flow_cache.append(&out.flow_kv_new);
            mimi_cache.append(&out.mimi_kv_new);
            mimi_conv = out.mimi_conv_out;
            mimi_offset = out.mimi_offset_out;
            latent = out.next_latent;
            if eos_frame.is_none() && out.eos_logit > EOS_THRESHOLD {
                eos_frame = Some(frame);
            }
            if let Some(eos) = eos_frame
                && frame >= eos + FRAMES_AFTER_EOS
            {
                break;
            }
            audio.extend_from_slice(&out.audio);
        }
        Ok(audio)
    }

    /// Latent step with sampled noise (split out so `run_step` stays the
    /// zero-noise prefill path).
    fn run_step_with_noise(
        &mut self,
        latent: &[f32],
        is_bos: f32,
        flow_cache: &mut Cache,
        mimi_cache: &mut Cache,
        mimi_offset: i64,
        mimi_conv: &[f32],
    ) -> Result<StepOutput, EngineError> {
        let noise = self.rng.noise(self.config.latent_dim, TEMPERATURE);
        let cfg = &self.config;
        let flow_window = flow_cache.window().to_vec();
        let flow_past = flow_cache.length;
        let mimi_window = mimi_cache.window_sized(cfg.mimi_kv_len);
        let inputs: Vec<(Cow<'_, str>, SessionInputValue<'_>)> = vec![
            (
                Cow::Borrowed("tokens"),
                tensor_i64(vec![1, 1], vec![0])?.into(),
            ),
            (
                Cow::Borrowed("latent"),
                tensor_f32(vec![1, 1, cfg.latent_dim], latent.to_vec())?.into(),
            ),
            (
                Cow::Borrowed("is_bos"),
                tensor_f32(vec![1, 1, 1], vec![is_bos])?.into(),
            ),
            (
                Cow::Borrowed("cond"),
                tensor_f32(vec![1, 1, cfg.model_dim], vec![0.0; cfg.model_dim])?.into(),
            ),
            (
                Cow::Borrowed("gates"),
                tensor_f32(vec![3], LATENT_GATE.to_vec())?.into(),
            ),
            (
                Cow::Borrowed("noise"),
                tensor_f32(vec![1, cfg.latent_dim], noise)?.into(),
            ),
            (
                Cow::Borrowed("flow_kv"),
                tensor_f32(
                    vec![
                        flow_past,
                        cfg.flow_layers,
                        2,
                        1,
                        cfg.flow_heads,
                        cfg.flow_head_dim,
                    ],
                    flow_window,
                )?
                .into(),
            ),
            (
                Cow::Borrowed("flow_offset"),
                tensor_i64(vec![], vec![flow_past as i64])?.into(),
            ),
            (
                Cow::Borrowed("mimi_kv"),
                tensor_f32(
                    vec![
                        cfg.mimi_kv_len,
                        cfg.mimi_layers,
                        2,
                        1,
                        cfg.mimi_heads,
                        cfg.mimi_head_dim,
                    ],
                    mimi_window,
                )?
                .into(),
            ),
            (
                Cow::Borrowed("mimi_offset"),
                tensor_i64(vec![], vec![mimi_offset])?.into(),
            ),
            (
                Cow::Borrowed("mimi_conv"),
                tensor_f32(vec![cfg.conv_state_size], mimi_conv.to_vec())?.into(),
            ),
            (
                Cow::Borrowed("decode_steps"),
                tensor_f32(vec![], vec![cfg.sampler_decode_steps])?.into(),
            ),
        ];
        let outputs = self
            .session
            .run(SessionInputs::<'_, '_, 0>::ValueMap(inputs))
            .map_err(|err| EngineError::PocketData(format!("model step failed: {err}")))?;
        let eos_values = extract_f32(&outputs, "eos_logit")?;
        Ok(StepOutput {
            audio: extract_f32(&outputs, "audio")?,
            next_latent: extract_f32(&outputs, "next_latent")?,
            eos_logit: eos_values.first().copied().unwrap_or(f32::NEG_INFINITY),
            flow_kv_new: extract_f32(&outputs, "flow_kv_new")?,
            mimi_kv_new: extract_f32(&outputs, "mimi_kv_new")?,
            mimi_offset_out: {
                let value = outputs.get("mimi_offset_out").ok_or_else(|| {
                    EngineError::PocketData("model gave no 'mimi_offset_out' output".to_string())
                })?;
                let (_, data) = value.try_extract_tensor::<i64>().map_err(|err| {
                    EngineError::PocketData(format!("model output 'mimi_offset_out': {err}"))
                })?;
                data.first().copied().unwrap_or(0)
            },
            mimi_conv_out: extract_f32(&outputs, "mimi_conv_out")?,
        })
    }

    /// Generate a chunk with runaway recovery: retry once, then split.
    fn generate_chunk(&mut self, text: &str, depth: usize) -> Result<Vec<f32>, EngineError> {
        let tokens = self.tokenizer.encode(text).len();
        let cap_seconds = ((tokens as f64 / self.config.tokens_per_second_estimate
            + self.config.gen_seconds_padding)
            * self.config.frame_rate)
            .ceil()
            / self.config.frame_rate;
        let limit = cap_seconds * RUNAWAY_RATIO;
        let sample_rate = f64::from(self.config.sample_rate);
        let mut shortest: Option<Vec<f32>> = None;
        for _ in 0..=1 {
            let audio = self.generate_once(text)?;
            if audio.len() as f64 / sample_rate <= limit {
                return Ok(audio);
            }
            if shortest
                .as_ref()
                .is_none_or(|best| audio.len() < best.len())
            {
                shortest = Some(audio);
            }
        }
        let words: Vec<&str> = text.split_whitespace().collect();
        if depth >= MAX_SPLIT_DEPTH || words.len() < 4 {
            eprintln!("Pocket TTS: chunk still hit the length cap after splitting: {text}");
            return Ok(shortest.unwrap_or_default());
        }
        eprintln!(
            "Pocket TTS: chunk hit the length cap; splitting {} words",
            words.len()
        );
        let half = words.len() / 2;
        let seam = vec![0.0; (SPLIT_SEAM.as_secs_f64() * sample_rate) as usize];
        let mut out = self.generate_chunk(&words[..half].join(" "), depth + 1)?;
        out.extend_from_slice(&seam);
        out.extend_from_slice(&self.generate_chunk(&words[half..].join(" "), depth + 1)?);
        Ok(out)
    }

    /// Synthesize `text` at `speed`, returning mono samples at 24 kHz.
    ///
    /// `speed` is clamped to 0.5..=2.0 (non-finite values become 1.0) and
    /// applied as a resample, so pitch moves with speed.
    pub fn synthesize(&mut self, text: &str, speed: f32) -> Result<Vec<f32>, EngineError> {
        let text = text.replace('\0', "");
        let normalized = normalize(&text);
        if normalized.is_empty() {
            return Err(EngineError::NoAudio(text));
        }
        let counter = |s: &str| self.tokenizer.encode(s).len();
        let chunks = split_text(&normalized, &counter, MAX_TOKENS_PER_CHUNK);
        if chunks.is_empty() {
            return Err(EngineError::NoAudio(text));
        }
        let sample_rate = self.config.sample_rate;
        let speed = if speed.is_finite() {
            speed.clamp(0.5, 2.0)
        } else {
            1.0
        };
        let mut pieces = Vec::with_capacity(chunks.len());
        for chunk in &chunks {
            let audio = self.generate_chunk(chunk, 0)?;
            pieces.push(RawPiece {
                audio,
                end: classify_piece_end(chunk),
            });
        }
        let audio = finish_speech(&pieces, sample_rate, speed, None);
        if audio.is_empty() {
            return Err(EngineError::NoAudio(text));
        }
        Ok(audio)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f16_decoding_covers_zero_one_and_subnormals() {
        assert_eq!(f16_to_f32(0x0000), 0.0);
        assert_eq!(f16_to_f32(0x3c00), 1.0);
        assert_eq!(f16_to_f32(0xbc00), -1.0);
        assert!((f16_to_f32(0x0001) - 5.960_464_5e-8).abs() < 1e-12);
        assert!(f16_to_f32(0x7e00).is_nan());
    }

    #[test]
    fn split_text_respects_token_budget_and_punctuation() {
        // One token per word in this toy counter.
        let counter = |s: &str| s.split_whitespace().count();
        let text = "یک دو سه چهار. پنج شش هفت هشت نه ده یازده دوازده سیزده چهارده پانزده شانزده هفده هجده نوزده بیست.";
        let chunks = split_text(text, &counter, 8);
        assert!(chunks.len() >= 3, "{chunks:?}");
        for chunk in &chunks {
            assert!(counter(chunk) <= 8, "{chunk}");
        }
        // Short text stays whole.
        assert_eq!(split_text("سلام دنیا.", &counter, 18), vec!["سلام دنیا."]);
        assert!(split_text("", &counter, 18).is_empty());
    }

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
    fn piece_end_rules() {
        assert_eq!(classify_piece_end("سلام."), PieceEnd::Sentence);
        assert_eq!(classify_piece_end("سلام!"), PieceEnd::Sentence);
        assert_eq!(classify_piece_end("سلام؟"), PieceEnd::Sentence);
        assert_eq!(classify_piece_end("salam?"), PieceEnd::Sentence);
        assert_eq!(classify_piece_end("سلام،"), PieceEnd::Clause);
        assert_eq!(classify_piece_end("سلام؛"), PieceEnd::Clause);
        assert_eq!(classify_piece_end("گفت:"), PieceEnd::Clause);
        assert_eq!(classify_piece_end("a,b"), PieceEnd::Mid);
        assert_eq!(classify_piece_end("a,"), PieceEnd::Clause);
        assert_eq!(classify_piece_end("a;"), PieceEnd::Clause);
        assert_eq!(classify_piece_end("نیمه"), PieceEnd::Mid);
        assert_eq!(classify_piece_end(""), PieceEnd::Mid);
        assert_eq!(trim_keep_ms(PieceEnd::Sentence), 120);
        assert_eq!(trim_keep_ms(PieceEnd::Clause), 60);
        assert_eq!(trim_keep_ms(PieceEnd::Mid), 25);
        assert_eq!(pause_after_ms(PieceEnd::Sentence), 150);
        assert_eq!(pause_after_ms(PieceEnd::Clause), 60);
        assert_eq!(pause_after_ms(PieceEnd::Mid), 0);
    }

    #[test]
    fn mastering_lifts_quiet_audio_and_limits_peaks() {
        // Quiet sine gains up toward the target.
        let quiet: Vec<f32> = (0..24_000)
            .map(|i| 0.1 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 24_000.0).sin())
            .collect();
        let lifted = master_chunk(&quiet, 24_000);
        assert_eq!(lifted.len(), quiet.len());
        let rms = |v: &[f32]| {
            (v.iter().map(|s| f64::from(*s) * f64::from(*s)).sum::<f64>() / v.len() as f64).sqrt()
        };
        assert!(
            rms(&lifted) > rms(&quiet) * 1.5,
            "quiet audio must get louder"
        );
        // Loud audio never exceeds the ceiling.
        let loud: Vec<f32> = (0..24_000)
            .map(|i| 0.99 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 24_000.0).sin())
            .collect();
        let limited = master_chunk(&loud, 24_000);
        let peak = limited.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(peak <= PEAK_CEILING, "peak {peak}");
        // Silence passes through.
        assert_eq!(master_chunk(&[0.0; 16], 24_000), vec![0.0; 16]);
        assert_eq!(measure_lufs(&[0.0; 4096], 24_000), f64::NEG_INFINITY);
    }

    /// Read a mono 16-bit PCM WAV fixture into f32 samples plus its rate.
    fn read_mono_pcm16_wav(path: &str) -> (Vec<f32>, u32) {
        let bytes = std::fs::read(path).unwrap();
        assert!(bytes.len() > 44, "too short for a WAV header");
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        let mut offset = 12;
        let mut rate = None;
        let mut samples = None;
        while offset + 8 <= bytes.len() {
            let id = &bytes[offset..offset + 4];
            let size =
                u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
            let body = &bytes[offset + 8..];
            if id == b"fmt " {
                let format = u16::from_le_bytes(body[0..2].try_into().unwrap());
                let channels = u16::from_le_bytes(body[2..4].try_into().unwrap());
                let bits = u16::from_le_bytes(body[14..16].try_into().unwrap());
                assert_eq!((format, channels, bits), (1, 1, 16), "need mono PCM16");
                rate = Some(u32::from_le_bytes(body[4..8].try_into().unwrap()));
            } else if id == b"data" {
                assert_eq!(size % 2, 0);
                samples = Some(
                    body[..size]
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|pair| {
                            let value = i16::from_le_bytes(*pair);
                            f64::from(value) as f32 / 32768.0
                        })
                        .collect(),
                );
            }
            offset += 8 + size + (size % 2);
        }
        (samples.expect("WAV has no data chunk"), rate.expect("WAV has no fmt chunk"))
    }

    #[test]
    fn fixture_masters_to_target_loudness() {
        let (raw, rate) = read_mono_pcm16_wav("tests/fixtures/pocket/raw-v1-6s.wav");
        assert_eq!((raw.len(), rate), (144_000, 24_000));
        let mastered = master_chunk(&raw, rate);
        assert_eq!(mastered.len(), raw.len());
        let lufs = measure_lufs(&mastered, rate);
        assert!(
            (lufs - TARGET_LUFS).abs() <= 1.0,
            "mastered fixture at {lufs} LUFS, want {TARGET_LUFS} ± 1"
        );
        let peak = mastered.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(peak <= PEAK_CEILING, "peak {peak}");
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
        // 60 ms; last end 120 ms. Pauses: 150 ms after the sentence piece,
        // 60 ms after the clause piece, none after the mid piece.
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
    fn speed_resample_changes_length_not_content_endpoints() {
        let samples: Vec<f32> = (0..100).map(|i| i as f32).collect();
        assert_eq!(apply_speed(&samples, 1.0).len(), 100);
        assert_eq!(apply_speed(&samples, 2.0).len(), 50);
        assert_eq!(apply_speed(&samples, 0.5).len(), 200);
    }

    #[test]
    fn rng_noise_has_plausible_spread() {
        let mut rng = Rng::new();
        let noise = rng.noise(4_000, 0.3);
        let mean = noise.iter().sum::<f32>() / noise.len() as f32;
        let var = noise.iter().map(|n| (n - mean) * (n - mean)).sum::<f32>() / noise.len() as f32;
        assert!(mean.abs() < 0.05, "mean {mean}");
        assert!((var - 0.3).abs() < 0.05, "variance {var}");
    }

    #[test]
    fn missing_model_names_the_onnx_path() {
        let dir =
            std::env::temp_dir().join(format!("smart-voices-pocket-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let result = PocketEngine::load(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
        match result {
            Err(EngineError::MissingModel(path)) => {
                assert_eq!(path, dir.join(POCKET_DIR).join(POCKET_FILE));
            }
            Err(other) => panic!("expected MissingModel, got {other}"),
            Ok(_) => panic!("expected MissingModel, got Ok"),
        }
    }
}
