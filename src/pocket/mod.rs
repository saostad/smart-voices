//! Pocket TTS (Farsi) engine on ONNX Runtime.
//!
//! The exported `.onnx` is one streaming step: text, voice conditioning, or
//! the previous latent goes in; one 80 ms audio frame plus the updated caches
//! come out. This module drives that graph the way the pocket-tts-onnx
//! runtimes do (`tts.py` / `tts.ts`): [`PocketEngine`] holds load, voice and
//! text prefill, the frame loop, and the chunk recipe (retry/split). The text
//! recipe lives in [`text`], the audio recipe in [`audio`], mastering in
//! [`mastering`], the ONNX Runtime setup in [`runtime`], and the tokenizer
//! and normalizer in [`sentencepiece`] and [`normalize_fa`].
//!
//! Runtime sharing: sherpa-onnx is linked shared, so its `libonnxruntime`
//! sits next to the executable; `ort` (load-dynamic, API 28) loads that same
//! library — one ONNX Runtime in the process. If the library or the model
//! fails to load, the error says so; nothing falls back to Piper.

mod audio;
mod mastering;
pub mod normalize_fa;
mod runtime;
pub mod sentencepiece;
mod text;

use std::borrow::Cow;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ort::session::{Session, SessionInputValue, SessionInputs};
use serde::Deserialize;

use crate::engine::EngineError;

use self::audio::{RawPiece, finish_speech};
use self::normalize_fa::normalize;
use self::runtime::{
    decode_f16_base64, extract_f32, ort_environment, tensor_f32, tensor_i64, thread_count,
};
use self::sentencepiece::SentencePiece;
use self::text::{classify_piece_end, prepare_text_prompt, split_text};

/// Directory under the models root holding the Pocket model.
const POCKET_DIR: &str = "pocket-farsi-v1";
/// The single exported model file inside [`POCKET_DIR`].
const POCKET_FILE: &str = "pocket-farsi-v1.onnx";

/// Farsi-verified generation settings (model card + blind-test harness).
const TEMPERATURE: f32 = 0.3;
const EOS_THRESHOLD: f32 = -2.0;
const FRAMES_AFTER_EOS: usize = 0;
const MAX_TOKENS_PER_CHUNK: usize = 18;
const SPLIT_SEAM: Duration = Duration::from_millis(50);
/// A generation longer than this share of its length cap never said EOS.
const RUNAWAY_RATIO: f64 = 0.97;
const MAX_SPLIT_DEPTH: usize = 2;

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
