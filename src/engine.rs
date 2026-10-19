//! Kokoro voice engine (sherpa-onnx) wrapper.

use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sherpa_onnx::{
    GenerationConfig, OfflineTts, OfflineTtsConfig, OfflineTtsKokoroModelConfig,
    OfflineTtsModelConfig, OfflineTtsVitsModelConfig,
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

/// Persian voices: Pocket (best quality, default) plus the Piper voices.
/// Pocket is one shared model; each Piper voice is its own model.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersianVoice {
    /// Pocket TTS Farsi (default, best quality).
    #[default]
    Pocket,
    /// `amir`.
    Amir,
    /// `gyro`.
    Gyro,
    /// `reza_ibrahim`.
    RezaIbrahim,
    /// `ganji`.
    Ganji,
    /// `ganji_adabi`.
    GanjiAdabi,
}

/// All Persian voices, in menu order (Pocket first: it is the default).
pub const PERSIAN_VOICES: [PersianVoice; 6] = [
    PersianVoice::Pocket,
    PersianVoice::Amir,
    PersianVoice::Gyro,
    PersianVoice::RezaIbrahim,
    PersianVoice::Ganji,
    PersianVoice::GanjiAdabi,
];

impl PersianVoice {
    /// Parse a CLI value or settings id (`"pocket"`, `"amir"`, `"gyro"`,
    /// `"reza_ibrahim"`, `"ganji"`, `"ganji_adabi"`).
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pocket" => Some(Self::Pocket),
            "amir" => Some(Self::Amir),
            "gyro" => Some(Self::Gyro),
            "reza_ibrahim" => Some(Self::RezaIbrahim),
            "ganji" => Some(Self::Ganji),
            "ganji_adabi" => Some(Self::GanjiAdabi),
            _ => None,
        }
    }

    /// Short id used in settings, the CLI, and file names.
    pub fn id(self) -> &'static str {
        match self {
            Self::Pocket => "pocket",
            Self::Amir => "amir",
            Self::Gyro => "gyro",
            Self::RezaIbrahim => "reza_ibrahim",
            Self::Ganji => "ganji",
            Self::GanjiAdabi => "ganji_adabi",
        }
    }

    /// Human-readable label for the Settings menu. Pocket says what it does
    /// for a beginner; the Piper names are just names.
    pub fn label(self) -> &'static str {
        match self {
            Self::Pocket => "Pocket (best quality — recommended)",
            Self::Amir => "Amir (Piper — smaller, faster)",
            Self::Gyro => "Gyro (Piper — smaller, faster)",
            Self::RezaIbrahim => "Reza Ibrahim (Piper — smaller, faster)",
            Self::Ganji => "Ganji (Piper — smaller, faster)",
            Self::GanjiAdabi => "Ganji Adabi (Piper — smaller, faster)",
        }
    }

    /// Directory name under the models root.
    pub fn dir_name(self) -> &'static str {
        match self {
            Self::Pocket => "pocket-farsi-v1",
            Self::Amir => "vits-piper-fa_IR-amir-medium",
            Self::Gyro => "vits-piper-fa_IR-gyro-medium",
            Self::RezaIbrahim => "vits-piper-fa_IR-reza_ibrahim-medium",
            Self::Ganji => "vits-piper-fa_IR-ganji-medium",
            Self::GanjiAdabi => "vits-piper-fa_IR-ganji_adabi-medium",
        }
    }

    /// ONNX model file name inside the model dir.
    pub fn model_file(self) -> &'static str {
        match self {
            Self::Pocket => "pocket-farsi-v1.onnx",
            Self::Amir => "fa_IR-amir-medium.onnx",
            Self::Gyro => "fa_IR-gyro-medium.onnx",
            Self::RezaIbrahim => "fa_IR-reza_ibrahim-medium.onnx",
            Self::Ganji => "fa_IR-ganji-medium.onnx",
            Self::GanjiAdabi => "fa_IR-ganji_adabi-medium.onnx",
        }
    }

    /// Download URL. Pocket is a single `.onnx` file on Hugging Face; the
    /// Piper voices are `.tar.bz2` archives (see `models::download`).
    pub fn url(self) -> &'static str {
        match self {
            Self::Pocket => {
                "https://huggingface.co/saostad/readly-voices/resolve/main/pocket-farsi-v1/pocket-farsi-v1.onnx"
            }
            Self::Amir => {
                "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/vits-piper-fa_IR-amir-medium.tar.bz2"
            }
            Self::Gyro => {
                "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/vits-piper-fa_IR-gyro-medium.tar.bz2"
            }
            Self::RezaIbrahim => {
                "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/vits-piper-fa_IR-reza_ibrahim-medium.tar.bz2"
            }
            Self::Ganji => {
                "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/vits-piper-fa_IR-ganji-medium.tar.bz2"
            }
            Self::GanjiAdabi => {
                "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/vits-piper-fa_IR-ganji_adabi-medium.tar.bz2"
            }
        }
    }

    /// Files and dirs that must exist inside the model dir (model file
    /// first). Pocket is a single self-contained file; Piper needs its
    /// tokens and espeak data too.
    pub fn required_files(self) -> Vec<&'static str> {
        match self {
            Self::Pocket => vec![self.model_file()],
            _ => vec![self.model_file(), "tokens.txt", "espeak-ng-data"],
        }
    }

    /// True for the Piper voices (Pocket is handled by its own engine).
    pub fn is_piper(self) -> bool {
        !matches!(self, Self::Pocket)
    }

    /// Expected size of the Pocket v1 `.onnx` download, in bytes.
    pub const POCKET_SIZE_BYTES: u64 = 226_396_445;
    /// Expected SHA-256 (hex) of the Pocket v1 `.onnx` download.
    pub const POCKET_SHA256: &str =
        "3f3322396a6f8a65e970438f0a58b146e90cd49283cf8c46cef7f3f877cdca30";
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

/// Human-readable name for a Kokoro voice id (`"af_heart"` →
/// `"Heart — American female"`), or `None` if the id is invalid. Port of
/// Swift's `KokoroVoiceCatalog.displayName`.
pub fn voice_display_name(id: &str) -> Option<String> {
    let mut chars = id.chars();
    let first = chars.next()?;
    let second = chars.next()?;
    if chars.next() != Some('_') {
        return None;
    }
    let (region, gender) = match (first, second) {
        ('a', 'f') => ("American", "female"),
        ('a', 'm') => ("American", "male"),
        ('b', 'f') => ("British", "female"),
        ('b', 'm') => ("British", "male"),
        _ => return None,
    };
    let suffix: String = chars.collect();
    if suffix.is_empty() {
        return None;
    }
    let mut suffix_chars = suffix.chars();
    let capitalized: String = suffix_chars.next()?.to_uppercase().collect();
    let rest: String = suffix_chars.collect();
    Some(format!("{capitalized}{rest} — {region} {gender}"))
}

/// Errors from loading the model or synthesizing speech.
#[derive(Debug)]
pub enum EngineError {
    /// A required model file or directory is missing.
    MissingModel(PathBuf),
    /// The Kokoro model files exist but sherpa-onnx refused to load them.
    LoadFailed(PathBuf),
    /// The Persian model files exist but sherpa-onnx refused to load them.
    PersianLoadFailed(PathBuf),
    /// The Pocket model file exists but ONNX Runtime refused it (the real
    /// runtime error is kept).
    PocketLoadFailed(PathBuf, String),
    /// The ONNX Runtime library itself couldn't be loaded (real error kept).
    PocketRuntime(String),
    /// The Pocket model ran but its data or outputs were malformed.
    PocketData(String),
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
                "Voice model file not found: {}. Run `smart-voices download` to get it.",
                path.display()
            ),
            EngineError::LoadFailed(dir) => write!(
                f,
                "Couldn't load the Kokoro voice model from {}. The files may be damaged; delete the folder and run `smart-voices download` again.",
                dir.display()
            ),
            EngineError::PersianLoadFailed(dir) => write!(
                f,
                "Couldn't load the Persian voice model from {}. The files may be damaged; delete the folder and run `smart-voices download --persian` again.",
                dir.display()
            ),
            EngineError::PocketLoadFailed(dir, message) => write!(
                f,
                "Couldn't load the Pocket Persian voice from {}: {message}. The file may be damaged; delete the folder and run `smart-voices download --persian` again.",
                dir.display()
            ),
            EngineError::PocketRuntime(message) => write!(
                f,
                "Couldn't load the ONNX Runtime library for the Pocket voice: {message}"
            ),
            EngineError::PocketData(message) => {
                write!(f, "The Pocket voice model gave bad data: {message}")
            }
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

/// Synthesis thread count shared by both engines: available parallelism
/// clamped to 1..=8.
fn thread_count() -> i32 {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, 8) as i32
}

/// Clamp `speed` to 0.5..=2.0 (non-finite values become 1.0).
fn clamp_speed(speed: f32) -> f32 {
    if speed.is_finite() {
        speed.clamp(0.5, 2.0)
    } else {
        1.0
    }
}

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
        let num_threads = thread_count();
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
        let speed = clamp_speed(speed);
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

/// Loaded Persian Piper (VITS) TTS engine. Each voice is its own single-speaker
/// model, so synthesis always uses sid 0.
pub struct PiperEngine {
    tts: OfflineTts,
}

impl PiperEngine {
    /// Load the Persian voice model from `models_root/<voice dir>`.
    pub fn load(models_root: &Path, voice: PersianVoice) -> Result<Self, EngineError> {
        let dir = models_root.join(voice.dir_name());
        // The model file is checked first so a bare directory reports the
        // most actionable missing path.
        for name in voice.required_files() {
            let path = dir.join(name);
            if !path.exists() {
                return Err(EngineError::MissingModel(path));
            }
        }

        let path_string = |name: &str| dir.join(name).to_string_lossy().into_owned();
        let config = OfflineTtsConfig {
            model: OfflineTtsModelConfig {
                vits: OfflineTtsVitsModelConfig {
                    model: Some(path_string(voice.model_file())),
                    tokens: Some(path_string("tokens.txt")),
                    data_dir: Some(path_string("espeak-ng-data")),
                    ..Default::default()
                },
                num_threads: thread_count(),
                provider: Some("cpu".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        match OfflineTts::create(&config) {
            Some(tts) => Ok(Self { tts }),
            None => Err(EngineError::PersianLoadFailed(dir)),
        }
    }

    /// Output sample rate in Hz.
    pub fn sample_rate(&self) -> i32 {
        self.tts.sample_rate()
    }

    /// Synthesize `text` at `speed`, returning mono samples.
    ///
    /// `speed` is clamped to 0.5..=2.0 (non-finite values become 1.0).
    pub fn synthesize(&self, text: &str, speed: f32) -> Result<Vec<f32>, EngineError> {
        // The crate passes text through CString::new().unwrap(), which would
        // abort the process (panic = "abort") on an interior NUL.
        let text = text.replace('\0', "");
        let gen_config = GenerationConfig {
            sid: 0,
            speed: clamp_speed(speed),
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
    fn voice_display_names_match_catalog_rule() {
        assert_eq!(
            voice_display_name("af_heart").as_deref(),
            Some("Heart — American female")
        );
        assert_eq!(
            voice_display_name("am_adam").as_deref(),
            Some("Adam — American male")
        );
        assert_eq!(
            voice_display_name("bf_alice").as_deref(),
            Some("Alice — British female")
        );
        assert_eq!(
            voice_display_name("bm_lewis").as_deref(),
            Some("Lewis — British male")
        );
        // Every bundled voice id has a display name.
        for voice in VOICES {
            assert!(voice_display_name(voice).is_some(), "{voice}");
        }
    }

    #[test]
    fn voice_display_name_rejects_invalid_ids() {
        for id in [
            "", "af", "af_", "afheart", "xf_heart", "ax_heart", "a_heart",
        ] {
            assert_eq!(voice_display_name(id), None, "{id}");
        }
    }

    #[test]
    fn missing_model_message_names_download_command() {
        let err = EngineError::MissingModel(PathBuf::from("/x/model.onnx"));
        assert_eq!(
            err.to_string(),
            "Voice model file not found: /x/model.onnx. Run `smart-voices download` to get it."
        );
    }

    #[test]
    fn persian_voice_ids_labels_and_paths() {
        let expected = [
            (
                PersianVoice::Pocket,
                "pocket",
                "Pocket (best quality — recommended)",
                "pocket-farsi-v1",
                "pocket-farsi-v1.onnx",
            ),
            (
                PersianVoice::Amir,
                "amir",
                "Amir (Piper — smaller, faster)",
                "vits-piper-fa_IR-amir-medium",
                "fa_IR-amir-medium.onnx",
            ),
            (
                PersianVoice::Gyro,
                "gyro",
                "Gyro (Piper — smaller, faster)",
                "vits-piper-fa_IR-gyro-medium",
                "fa_IR-gyro-medium.onnx",
            ),
            (
                PersianVoice::RezaIbrahim,
                "reza_ibrahim",
                "Reza Ibrahim (Piper — smaller, faster)",
                "vits-piper-fa_IR-reza_ibrahim-medium",
                "fa_IR-reza_ibrahim-medium.onnx",
            ),
            (
                PersianVoice::Ganji,
                "ganji",
                "Ganji (Piper — smaller, faster)",
                "vits-piper-fa_IR-ganji-medium",
                "fa_IR-ganji-medium.onnx",
            ),
            (
                PersianVoice::GanjiAdabi,
                "ganji_adabi",
                "Ganji Adabi (Piper — smaller, faster)",
                "vits-piper-fa_IR-ganji_adabi-medium",
                "fa_IR-ganji_adabi-medium.onnx",
            ),
        ];
        assert_eq!(PERSIAN_VOICES.len(), expected.len());
        for (voice, id, label, dir, file) in expected {
            assert!(PERSIAN_VOICES.contains(&voice), "{id}");
            assert_eq!(voice.id(), id);
            assert_eq!(PersianVoice::parse(id), Some(voice));
            assert_eq!(voice.label(), label);
            assert_eq!(voice.dir_name(), dir);
            assert_eq!(voice.model_file(), file);
            if voice == PersianVoice::Pocket {
                assert_eq!(voice.required_files(), [file]);
                assert_eq!(
                    voice.url(),
                    "https://huggingface.co/saostad/readly-voices/resolve/main/pocket-farsi-v1/pocket-farsi-v1.onnx"
                );
                assert_eq!(PersianVoice::POCKET_SIZE_BYTES, 226_396_445);
                assert_eq!(
                    PersianVoice::POCKET_SHA256,
                    "3f3322396a6f8a65e970438f0a58b146e90cd49283cf8c46cef7f3f877cdca30"
                );
            } else {
                assert_eq!(
                    voice.required_files(),
                    [file, "tokens.txt", "espeak-ng-data"]
                );
                assert_eq!(
                    voice.url(),
                    format!(
                        "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/{dir}.tar.bz2"
                    )
                );
            }
        }
        assert_eq!(PersianVoice::parse("sara"), None);
        assert_eq!(PersianVoice::parse(""), None);
        assert_eq!(PersianVoice::default(), PersianVoice::Pocket);
        assert_eq!(PERSIAN_VOICES[0], PersianVoice::Pocket);
    }

    #[test]
    fn each_persian_voice_missing_model_names_its_onnx_path() {
        for voice in PERSIAN_VOICES.iter().filter(|v| v.is_piper()) {
            let dir = unique_temp_dir();
            std::fs::create_dir_all(&dir).unwrap();
            let result = PiperEngine::load(&dir, *voice);
            std::fs::remove_dir_all(&dir).unwrap();
            match result {
                Err(EngineError::MissingModel(path)) => {
                    assert_eq!(
                        path,
                        dir.join(voice.dir_name()).join(voice.model_file()),
                        "voice {voice:?}"
                    );
                }
                Ok(_) => panic!("expected MissingModel, got Ok ({voice:?})"),
                Err(other) => panic!("expected MissingModel, got {other} ({voice:?})"),
            }
        }
    }

    #[test]
    fn persian_load_failed_names_dir_and_persian_download() {
        let err =
            EngineError::PersianLoadFailed(PathBuf::from("/models/vits-piper-fa_IR-amir-medium"));
        assert_eq!(
            err.to_string(),
            "Couldn't load the Persian voice model from /models/vits-piper-fa_IR-amir-medium. The files may be damaged; delete the folder and run `smart-voices download --persian` again."
        );
    }
}
