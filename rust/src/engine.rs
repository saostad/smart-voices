//! Kokoro voice engine (sherpa-onnx) wrapper.

use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sherpa_onnx::{
    GenerationConfig, OfflineTts, OfflineTtsConfig, OfflineTtsKokoroModelConfig,
    OfflineTtsModelConfig,
};

/// Downloadable model variant: full-precision (fast) or int8 (small).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelVariant {
    /// Full-precision model: larger download, faster synthesis.
    #[default]
    Fast,
    /// Quantized int8 model: smaller download, slower synthesis.
    Small,
}

impl ModelVariant {
    /// Parse a CLI value (`"fast"` or `"small"`).
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "fast" => Some(Self::Fast),
            "small" => Some(Self::Small),
            _ => None,
        }
    }

    /// Directory name under the models root.
    pub fn dir_name(self) -> &'static str {
        match self {
            Self::Fast => "kokoro-multi-lang-v1_0",
            Self::Small => "kokoro-int8-multi-lang-v1_0",
        }
    }

    /// ONNX model file name inside the model dir.
    pub fn model_file(self) -> &'static str {
        match self {
            Self::Fast => "model.onnx",
            Self::Small => "model.int8.onnx",
        }
    }

    /// Download URL of the `.tar.bz2` archive.
    pub fn url(self) -> &'static str {
        match self {
            Self::Fast => {
                "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/kokoro-multi-lang-v1_0.tar.bz2"
            }
            Self::Small => {
                "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/kokoro-int8-multi-lang-v1_0.tar.bz2"
            }
        }
    }

    /// Human-readable label for download progress.
    pub fn label(self) -> &'static str {
        match self {
            Self::Fast => "Fast (larger download, ~350 MB)",
            Self::Small => "Small (slower, ~130 MB)",
        }
    }

    /// Files and dirs that must exist inside the model dir (model file first).
    pub fn required_files(self) -> [&'static str; 6] {
        [
            self.model_file(),
            "voices.bin",
            "tokens.txt",
            "espeak-ng-data",
            "dict",
            "lexicon-us-en.txt",
        ]
    }
}

/// English Kokoro voices in speaker-id order (sid = index).
pub const VOICES: [&str; 28] = [
    "af_alloy",
    "af_aoede",
    "af_bella",
    "af_heart",
    "af_jessica",
    "af_kore",
    "af_nicole",
    "af_nova",
    "af_river",
    "af_sarah",
    "af_sky",
    "am_adam",
    "am_echo",
    "am_eric",
    "am_fenrir",
    "am_liam",
    "am_michael",
    "am_onyx",
    "am_puck",
    "am_santa",
    "bf_alice",
    "bf_emma",
    "bf_isabella",
    "bf_lily",
    "bm_daniel",
    "bm_fable",
    "bm_george",
    "bm_lewis",
];

/// Default voice.
pub const DEFAULT_VOICE: &str = "af_heart";

/// Look up a voice name, returning its speaker id (index into [`VOICES`]).
pub fn voice_id(name: &str) -> Option<i32> {
    VOICES.iter().position(|v| *v == name).map(|i| i as i32)
}

/// Errors from loading the model or synthesizing speech.
#[derive(Debug)]
pub enum EngineError {
    /// A required model file or directory is missing.
    MissingModel(PathBuf),
    /// The model files exist but sherpa-onnx refused to load them.
    LoadFailed(PathBuf),
    /// The requested voice name is not in [`VOICES`].
    UnknownVoice(String),
    /// Synthesis returned no audio samples.
    NoAudio(String),
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EngineError::MissingModel(path) => write!(
                f,
                "Voice model file not found: {}. Run rust/scripts/fetch-models.sh to download it.",
                path.display()
            ),
            EngineError::LoadFailed(dir) => write!(
                f,
                "Couldn't load the Kokoro voice model from {}. The files may be damaged; delete the folder and run rust/scripts/fetch-models.sh again.",
                dir.display()
            ),
            EngineError::UnknownVoice(name) => {
                write!(
                    f,
                    "Unknown voice '{name}'. Available voices: {}.",
                    VOICES.join(", ")
                )
            }
            EngineError::NoAudio(text) => {
                let preview: String = text.chars().take(60).collect();
                if text.chars().count() > 60 {
                    write!(f, "The voice produced no audio for: \"{preview}…\"")
                } else {
                    write!(f, "The voice produced no audio for: \"{preview}\"")
                }
            }
        }
    }
}

impl Error for EngineError {}

/// Loaded Kokoro TTS engine.
pub struct KokoroEngine {
    tts: OfflineTts,
}

impl KokoroEngine {
    /// Load the voice model variant from `models_root/<variant dir>`.
    pub fn load(models_root: &Path, variant: ModelVariant) -> Result<Self, EngineError> {
        let dir = models_root.join(variant.dir_name());
        // The model file is checked first so a bare directory reports the
        // most actionable missing path.
        for name in variant.required_files() {
            let path = dir.join(name);
            if !path.exists() {
                return Err(EngineError::MissingModel(path));
            }
        }

        let path_string = |name: &str| dir.join(name).to_string_lossy().into_owned();
        let num_threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .clamp(1, 8) as i32;
        let config = OfflineTtsConfig {
            model: OfflineTtsModelConfig {
                kokoro: OfflineTtsKokoroModelConfig {
                    model: Some(path_string(variant.model_file())),
                    voices: Some(path_string("voices.bin")),
                    tokens: Some(path_string("tokens.txt")),
                    data_dir: Some(path_string("espeak-ng-data")),
                    dict_dir: Some(path_string("dict")),
                    lexicon: Some(path_string("lexicon-us-en.txt")),
                    lang: Some("en-us".to_string()),
                    ..Default::default()
                },
                num_threads,
                provider: Some("cpu".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        match OfflineTts::create(&config) {
            Some(tts) => Ok(Self { tts }),
            None => Err(EngineError::LoadFailed(dir)),
        }
    }

    /// Output sample rate in Hz.
    pub fn sample_rate(&self) -> i32 {
        self.tts.sample_rate()
    }

    /// Synthesize `text` with `voice` at `speed`, returning mono samples.
    ///
    /// `speed` is clamped to 0.5..=2.0 (non-finite values become 1.0).
    pub fn synthesize(&self, text: &str, voice: &str, speed: f32) -> Result<Vec<f32>, EngineError> {
        let sid = voice_id(voice).ok_or_else(|| EngineError::UnknownVoice(voice.to_string()))?;
        // The crate passes text through CString::new().unwrap(), which would
        // abort the process (panic = "abort") on an interior NUL.
        let text = text.replace('\0', "");
        let speed = if speed.is_finite() {
            speed.clamp(0.5, 2.0)
        } else {
            1.0
        };
        let gen_config = GenerationConfig {
            sid,
            speed,
            ..Default::default()
        };
        let audio =
            self.tts
                .generate_with_config(&text, &gen_config, None::<fn(&[f32], f32) -> bool>);
        match audio {
            Some(audio) => {
                let samples = audio.samples().to_vec();
                if samples.is_empty() {
                    Err(EngineError::NoAudio(text))
                } else {
                    Ok(samples)
                }
            }
            None => Err(EngineError::NoAudio(text)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_temp_dir() -> PathBuf {
        let id = TEST_DIR_COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!("smart-voices-test-{}-{id}", std::process::id()))
    }

    #[test]
    fn voice_id_known_voices() {
        assert_eq!(voice_id("af_heart"), Some(3));
        assert_eq!(voice_id("bm_lewis"), Some(27));
        assert_eq!(voice_id("af_alloy"), Some(0));
    }

    #[test]
    fn unknown_voice_message_lists_voices() {
        let err = EngineError::UnknownVoice("jarvis".to_string());
        let msg = err.to_string();
        assert!(msg.contains("Unknown voice 'jarvis'"), "{msg}");
        for voice in VOICES {
            assert!(msg.contains(voice), "{msg}");
        }
    }

    #[test]
    fn load_on_empty_dir_reports_missing_model_file() {
        let dir = unique_temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let result = KokoroEngine::load(&dir, ModelVariant::Small);
        std::fs::remove_dir_all(&dir).unwrap();
        match result {
            Err(EngineError::MissingModel(path)) => {
                assert_eq!(
                    path,
                    dir.join(ModelVariant::Small.dir_name())
                        .join("model.int8.onnx")
                );
            }
            Ok(_) => panic!("expected MissingModel, got Ok"),
            Err(other) => panic!("expected MissingModel, got {other}"),
        }
    }

    #[test]
    fn each_variant_missing_model_names_its_own_model_file() {
        for variant in [ModelVariant::Fast, ModelVariant::Small] {
            let dir = unique_temp_dir();
            std::fs::create_dir_all(&dir).unwrap();
            let result = KokoroEngine::load(&dir, variant);
            std::fs::remove_dir_all(&dir).unwrap();
            match result {
                Err(EngineError::MissingModel(path)) => {
                    assert_eq!(
                        path,
                        dir.join(variant.dir_name()).join(variant.model_file()),
                        "variant {variant:?}"
                    );
                }
                Ok(_) => panic!("expected MissingModel, got Ok ({variant:?})"),
                Err(other) => panic!("expected MissingModel, got {other} ({variant:?})"),
            }
        }
    }

    #[test]
    fn missing_model_message_names_fetch_script() {
        let err = EngineError::MissingModel(PathBuf::from("/x/model.int8.onnx"));
        assert_eq!(
            err.to_string(),
            "Voice model file not found: /x/model.int8.onnx. Run rust/scripts/fetch-models.sh to download it."
        );
    }
}
