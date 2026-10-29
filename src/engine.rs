//! Voice model definitions and ONNX synthesis engines.

use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
/// Downloadable English model variant: full-precision Kokoro (fast), int8
/// Kokoro (small), or Phonon (best quality).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelVariant {
    /// Full-precision Kokoro model: larger download, faster synthesis.
    #[default]
    Fast,
    /// Quantized int8 Kokoro model: smaller download, slower synthesis.
    Small,
    /// Phonon-31m English: best quality, smaller download.
    Phonon,
}

impl ModelVariant {
    /// Parse a CLI value (`"fast"`, `"small"` or `"phonon"`).
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "fast" => Some(Self::Fast),
            "small" => Some(Self::Small),
            "phonon" => Some(Self::Phonon),
            _ => None,
        }
    }

    /// Directory name under the models root.
    pub fn dir_name(self) -> &'static str {
        match self {
            Self::Fast => "kokoro-multi-lang-v1_0",
            Self::Small => "kokoro-int8-multi-lang-v1_0",
            Self::Phonon => "phonon-31m-en",
        }
    }

    /// Model file name inside the model dir.
    pub fn model_file(self) -> &'static str {
        match self {
            Self::Fast => "model.onnx",
            Self::Small => "model.int8.onnx",
            Self::Phonon => "model.safetensors",
        }
    }

    /// Download URL: the `.tar.bz2` archive for the Kokoro variants, the
    /// model page for Phonon (its real URLs are per file; see
    /// [`download_files`](Self::download_files)). Used only in messages.
    pub fn url(self) -> &'static str {
        match self {
            Self::Fast => {
                "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/kokoro-multi-lang-v1_0.tar.bz2"
            }
            Self::Small => {
                "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/kokoro-int8-multi-lang-v1_0.tar.bz2"
            }
            Self::Phonon => "https://huggingface.co/gradium/phonon-31m-en",
        }
    }

    /// Human-readable label for download progress.
    pub fn label(self) -> &'static str {
        match self {
            Self::Fast => "Fast (larger download, ~350 MB)",
            Self::Small => "Small (slower, ~130 MB)",
            Self::Phonon => "Phonon (best quality, ~103 MB)",
        }
    }

    /// Files and dirs that must exist inside the model dir (model file first).
    pub fn required_files(self) -> &'static [&'static str] {
        match self {
            Self::Fast => &["model.onnx", "voices.bin", "tokens.txt", "espeak-ng-data"],
            Self::Small => &[
                "model.int8.onnx",
                "voices.bin",
                "tokens.txt",
                "espeak-ng-data",
            ],
            Self::Phonon => &[
                "model.safetensors",
                "config.json",
                "tokenizer.json",
                crate::phonon::DEFAULT_VOICE_FILE,
            ],
        }
    }

    /// Per-file downloads for variants that don't ship as one archive:
    /// empty for the Kokoro variants, the four Phonon files otherwise.
    pub fn download_files(self) -> &'static [DownloadFile] {
        match self {
            Self::Fast | Self::Small => &[],
            Self::Phonon => &[
                DownloadFile {
                    name: "model.safetensors",
                    spec: DownloadSpec {
                        url: "https://huggingface.co/gradium/phonon-31m-en/resolve/52433c46975bfa13144c56204c9f4ee1c5b148aa/model.safetensors",
                        size: 102_823_920,
                        sha256: "8a04cf833cfe69b67c2ec968b2d31244655b9accc84ade9c0ebcc176f2074607",
                    },
                },
                DownloadFile {
                    name: "config.json",
                    spec: DownloadSpec {
                        url: "https://huggingface.co/gradium/phonon-31m-en/resolve/52433c46975bfa13144c56204c9f4ee1c5b148aa/config.json",
                        size: 2851,
                        sha256: "7062ea376e6b158543c8abec8e5af67df171c49ad70eb7e2a0fb25c16f265e06",
                    },
                },
                DownloadFile {
                    name: "tokenizer.json",
                    spec: DownloadSpec {
                        url: "https://huggingface.co/gradium/phonon-31m-en/resolve/52433c46975bfa13144c56204c9f4ee1c5b148aa/tokenizer.json",
                        size: 277_967,
                        sha256: "691a00068fa7b989f2f3e5ae5daa2d1a5bb319f2d5360deafcfb19a8bb6e7936",
                    },
                },
                DownloadFile {
                    name: "default-voice.safetensors",
                    spec: DownloadSpec {
                        url: "https://huggingface.co/gradium/phonon-31m-en/resolve/52433c46975bfa13144c56204c9f4ee1c5b148aa/default-voice.safetensors",
                        size: 256_136,
                        sha256: "f739e8c4c76ee7f92e7c94a82002748c8cd22e99582ac49f7b9b94cd63786e18",
                    },
                },
            ],
        }
    }
}

/// Persian voices: Pocket v2 (best quality, default), Pocket v1, plus the
/// Piper voices. Each Pocket voice is one shared model; each Piper voice is
/// its own model.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersianVoice {
    /// Pocket TTS Farsi v2 (default, best quality).
    #[default]
    #[serde(rename = "pocket-v2")]
    PocketV2,
    /// Pocket TTS Farsi v1.
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

/// All Persian voices, in menu order (Pocket v2 first: it is the default).
pub const PERSIAN_VOICES: [PersianVoice; 7] = [
    PersianVoice::PocketV2,
    PersianVoice::Pocket,
    PersianVoice::Amir,
    PersianVoice::Gyro,
    PersianVoice::RezaIbrahim,
    PersianVoice::Ganji,
    PersianVoice::GanjiAdabi,
];

impl PersianVoice {
    /// Parse a CLI value or settings id (`"pocket-v2"`, `"pocket"`,
    /// `"amir"`, `"gyro"`, `"reza_ibrahim"`, `"ganji"`, `"ganji_adabi"`).
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pocket-v2" => Some(Self::PocketV2),
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
            Self::PocketV2 => "pocket-v2",
            Self::Pocket => "pocket",
            Self::Amir => "amir",
            Self::Gyro => "gyro",
            Self::RezaIbrahim => "reza_ibrahim",
            Self::Ganji => "ganji",
            Self::GanjiAdabi => "ganji_adabi",
        }
    }

    /// Short label for the `Persian Voice` submenu: just the name. The
    /// submenu's `Piper (smaller, faster)` header already says what Piper
    /// means; [`label`](Self::label) keeps the full text for the CLI and the
    /// status line.
    pub fn short_label(self) -> &'static str {
        match self {
            Self::PocketV2 => "Pocket v2",
            Self::Pocket => "Pocket v1",
            Self::Amir => "Amir",
            Self::Gyro => "Gyro",
            Self::RezaIbrahim => "Reza Ibrahim",
            Self::Ganji => "Ganji",
            Self::GanjiAdabi => "Ganji Adabi",
        }
    }

    /// Human-readable label for the Settings menu. Pocket v2 says what it
    /// does for a beginner; the Piper names are just names.
    pub fn label(self) -> &'static str {
        match self {
            Self::PocketV2 => "Pocket v2 (best quality — recommended)",
            Self::Pocket => "Pocket v1",
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
            Self::PocketV2 => "pocket-farsi-v2",
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
            Self::PocketV2 => "pocket-farsi-v2.onnx",
            Self::Pocket => "pocket-farsi-v1.onnx",
            Self::Amir => "fa_IR-amir-medium.onnx",
            Self::Gyro => "fa_IR-gyro-medium.onnx",
            Self::RezaIbrahim => "fa_IR-reza_ibrahim-medium.onnx",
            Self::Ganji => "fa_IR-ganji-medium.onnx",
            Self::GanjiAdabi => "fa_IR-ganji_adabi-medium.onnx",
        }
    }

    /// Download URL. Each Pocket voice is a single `.onnx` file on
    /// Hugging Face; the Piper voices are `.tar.bz2` archives (see
    /// `models::download`).
    pub fn url(self) -> &'static str {
        match self {
            Self::PocketV2 => {
                "https://huggingface.co/saostad/readly-voices/resolve/main/pocket-farsi-v2/pocket-farsi-v2.onnx"
            }
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
    /// first). Each Pocket voice is a single self-contained file; Piper
    /// needs its tokens and espeak data too.
    pub fn required_files(self) -> Vec<&'static str> {
        match self {
            Self::Pocket | Self::PocketV2 => vec![self.model_file()],
            _ => vec![
                self.model_file(),
                self.config_file(),
                "tokens.txt",
                "espeak-ng-data",
            ],
        }
    }

    /// Inference scales shipped alongside each Piper ONNX model.
    pub fn config_file(self) -> &'static str {
        match self {
            Self::Amir => "fa_IR-amir-medium.onnx.json",
            Self::Gyro => "fa_IR-gyro-medium.onnx.json",
            Self::RezaIbrahim => "fa_IR-reza_ibrahim-medium.onnx.json",
            Self::Ganji => "fa_IR-ganji-medium.onnx.json",
            Self::GanjiAdabi => "fa_IR-ganji_adabi-medium.onnx.json",
            Self::Pocket | Self::PocketV2 => "",
        }
    }

    /// True for the Piper voices (Pocket is handled by its own engine).
    pub fn is_piper(self) -> bool {
        !matches!(self, Self::Pocket | Self::PocketV2)
    }

    /// Single-file download coordinates for the Pocket voices (`None` for
    /// Piper, which ships as `.tar.bz2` archives). Adding a voice is one
    /// entry here (plus its `url`).
    pub fn download_spec(self) -> Option<DownloadSpec> {
        match self {
            Self::PocketV2 => Some(DownloadSpec {
                url: self.url(),
                size: 226_378_457,
                sha256: "53a49a5d2fdf94832a0163939c38eb638789ae1f1678dccdd539240dffcf4703",
            }),
            Self::Pocket => Some(DownloadSpec {
                url: self.url(),
                size: 226_396_445,
                sha256: "3f3322396a6f8a65e970438f0a58b146e90cd49283cf8c46cef7f3f877cdca30",
            }),
            _ => None,
        }
    }
}

/// One voice's single-file download: URL, expected size in bytes, and
/// expected SHA-256 (hex). See [`PersianVoice::download_spec`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DownloadSpec {
    pub url: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

/// One file of a multi-file model download: its name inside the model dir
/// plus its download coordinates. See [`ModelVariant::download_files`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DownloadFile {
    pub name: &'static str,
    pub spec: DownloadSpec,
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

/// English voice group from the id prefix (`af_` → American female, …).
/// Groups the `English Voice` submenu; see [`grouped_english_voices`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceGroup {
    /// `af_*` voices.
    AmericanFemale,
    /// `am_*` voices.
    AmericanMale,
    /// `bf_*` voices.
    BritishFemale,
    /// `bm_*` voices.
    BritishMale,
}

impl VoiceGroup {
    /// All groups in menu order.
    pub const ALL: [Self; 4] = [
        Self::AmericanFemale,
        Self::AmericanMale,
        Self::BritishFemale,
        Self::BritishMale,
    ];

    /// Parse the id prefix (`af_` → American female, …), or `None` if the
    /// id doesn't start with a known prefix.
    pub fn parse(id: &str) -> Option<Self> {
        let mut chars = id.chars();
        let first = chars.next()?;
        let second = chars.next()?;
        if chars.next() != Some('_') {
            return None;
        }
        match (first, second) {
            ('a', 'f') => Some(Self::AmericanFemale),
            ('a', 'm') => Some(Self::AmericanMale),
            ('b', 'f') => Some(Self::BritishFemale),
            ('b', 'm') => Some(Self::BritishMale),
            _ => None,
        }
    }

    /// Region word for [`voice_display_name`].
    fn region(self) -> &'static str {
        match self {
            Self::AmericanFemale | Self::AmericanMale => "American",
            Self::BritishFemale | Self::BritishMale => "British",
        }
    }

    /// Gender word for [`voice_display_name`].
    fn gender(self) -> &'static str {
        match self {
            Self::AmericanFemale | Self::BritishFemale => "female",
            Self::AmericanMale | Self::BritishMale => "male",
        }
    }

    /// Disabled-header label for the `English Voice` submenu.
    pub fn header(self) -> &'static str {
        match self {
            Self::AmericanFemale => "American · Female",
            Self::AmericanMale => "American · Male",
            Self::BritishFemale => "British · Female",
            Self::BritishMale => "British · Male",
        }
    }
}

/// Short display name for a Kokoro voice id (`"af_heart"` → `"Heart"`), or
/// `None` if the id is invalid. Shown under the group header, which already
/// carries the region and gender.
pub fn voice_short_name(id: &str) -> Option<String> {
    VoiceGroup::parse(id)?;
    let suffix = id.get(3..)?;
    if suffix.is_empty() {
        return None;
    }
    let mut chars = suffix.chars();
    let capitalized: String = chars.next()?.to_uppercase().collect();
    let rest: String = chars.collect();
    Some(format!("{capitalized}{rest}"))
}

/// Human-readable name for a Kokoro voice id (`"af_heart"` →
/// `"Heart — American female"`), or `None` if the id is invalid. Port of
/// Swift's `KokoroVoiceCatalog.displayName`.
pub fn voice_display_name(id: &str) -> Option<String> {
    let group = VoiceGroup::parse(id)?;
    let name = voice_short_name(id)?;
    Some(format!("{name} — {} {}", group.region(), group.gender()))
}

/// English voices grouped for the `English Voice` submenu: the four
/// [`VoiceGroup::ALL`] groups in menu order, ids A–Z by short name within
/// each group.
pub fn grouped_english_voices() -> [(VoiceGroup, Vec<&'static str>); 4] {
    VoiceGroup::ALL.map(|group| {
        let mut ids: Vec<&'static str> = VOICES
            .iter()
            .copied()
            .filter(|id| VoiceGroup::parse(id) == Some(group))
            .collect();
        ids.sort_unstable_by_key(|id| voice_short_name(id));
        (group, ids)
    })
}

/// Errors from loading the model or synthesizing speech.
#[derive(Debug)]
pub enum EngineError {
    OnnxTts(String),
    /// A required model file or directory is missing.
    MissingModel(PathBuf),
    /// The Kokoro model files exist but ONNX Runtime refused to load them.
    LoadFailed(PathBuf),
    /// The Persian model files exist but ONNX Runtime refused to load them.
    PersianLoadFailed(PathBuf),
    /// The Pocket model file exists but ONNX Runtime refused it (the real
    /// runtime error is kept).
    PocketLoadFailed(PathBuf, String),
    /// The ONNX Runtime library itself couldn't be loaded (real error kept).
    PocketRuntime(String),
    /// The Pocket model ran but its data or outputs were malformed.
    PocketData(String),
    /// The embedded Persian pronunciation (G2P) model for Pocket v2 failed
    /// to load (the real runtime error is kept).
    G2pLoadFailed(String),
    /// Pronouncing one sentence failed (sentence and real error kept).
    G2pFailed {
        sentence: String,
        message: String,
    },
    /// The Phonon model files exist but ptts refused to load them (the real
    /// error is kept).
    PhononLoadFailed(PathBuf, String),
    /// Phonon synthesis failed (the real error is kept).
    PhononFailed(String),
    /// The requested voice name is not in [`VOICES`].
    UnknownVoice(String),
    /// Synthesis returned no audio samples.
    NoAudio(String),
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EngineError::OnnxTts(message) => write!(f, "Voice synthesis failed: {message}"),
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
            EngineError::G2pLoadFailed(message) => write!(
                f,
                "Couldn't load the Persian pronunciation model: {message}"
            ),
            EngineError::G2pFailed { sentence, message } => {
                let preview: String = sentence.chars().take(40).collect();
                write!(f, "Pronunciation step failed for \"{preview}\": {message}")
            }
            EngineError::PhononLoadFailed(dir, message) => write!(
                f,
                "Couldn't load the Phonon voice model from {}: {message}. The files may be damaged; delete the folder and run `smart-voices download --model phonon` again.",
                dir.display()
            ),
            EngineError::PhononFailed(message) => {
                write!(f, "The Phonon voice failed: {message}")
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

/// Clamp `speed` to 0.5..=2.0 (non-finite values become 1.0).
pub(crate) fn clamp_speed(speed: f32) -> f32 {
    if speed.is_finite() {
        speed.clamp(0.5, 2.0)
    } else {
        1.0
    }
}

/// Loaded Kokoro TTS engine. Sessions and lazy regional lexicons are serialized.
pub struct KokoroEngine {
    sample_rate: i32,
    inner: std::sync::Mutex<crate::onnx_tts::Kokoro>,
}
impl KokoroEngine {
    pub fn load(root: &Path, variant: ModelVariant) -> Result<Self, EngineError> {
        for name in variant.required_files() {
            let path = root.join(variant.dir_name()).join(name);
            if !path.exists() {
                return Err(EngineError::MissingModel(path));
            }
        }
        crate::onnx_tts::Kokoro::load(root, variant)
            .map(|inner| Self {
                sample_rate: inner.sample_rate,
                inner: std::sync::Mutex::new(inner),
            })
            .map_err(tts_error)
    }
    pub fn sample_rate(&self) -> i32 {
        self.sample_rate
    }
    pub fn synthesize(&self, text: &str, voice: &str, speed: f32) -> Result<Vec<f32>, EngineError> {
        let sid = voice_id(voice).ok_or_else(|| EngineError::UnknownVoice(voice.into()))?;
        self.inner
            .lock()
            .map_err(|_| tts_error("voice session lock failed".into()))?
            .synthesize(
                text,
                sid as usize,
                voice.starts_with('b'),
                clamp_speed(speed),
            )
            .map_err(tts_error)
    }
}
/// Loaded Persian Piper VITS voice.
pub struct PiperEngine {
    sample_rate: i32,
    inner: std::sync::Mutex<crate::onnx_tts::Piper>,
}
impl PiperEngine {
    pub fn load(root: &Path, voice: PersianVoice) -> Result<Self, EngineError> {
        for name in voice.required_files() {
            let path = root.join(voice.dir_name()).join(name);
            if !path.exists() {
                return Err(EngineError::MissingModel(path));
            }
        }
        crate::onnx_tts::Piper::load(root, voice)
            .map(|inner| Self {
                sample_rate: inner.sample_rate,
                inner: std::sync::Mutex::new(inner),
            })
            .map_err(tts_error)
    }
    pub fn sample_rate(&self) -> i32 {
        self.sample_rate
    }
    pub fn synthesize(&self, text: &str, speed: f32) -> Result<Vec<f32>, EngineError> {
        self.inner
            .lock()
            .map_err(|_| tts_error("voice session lock failed".into()))?
            .synthesize(text, clamp_speed(speed))
            .map_err(tts_error)
    }
}
fn tts_error(message: String) -> EngineError {
    eprintln!("error: ONNX voice engine: {message}");
    EngineError::OnnxTts(message)
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
    fn voice_short_names_are_capitalized_suffixes() {
        assert_eq!(voice_short_name("af_heart").as_deref(), Some("Heart"));
        assert_eq!(voice_short_name("am_adam").as_deref(), Some("Adam"));
        assert_eq!(voice_short_name("bf_alice").as_deref(), Some("Alice"));
        assert_eq!(voice_short_name("bm_lewis").as_deref(), Some("Lewis"));
        for id in [
            "", "af", "af_", "afheart", "xf_heart", "ax_heart", "a_heart",
        ] {
            assert_eq!(voice_short_name(id), None, "{id}");
        }
    }

    #[test]
    fn english_voice_groups_cover_all_voices_sorted_a_to_z() {
        let groups = grouped_english_voices();
        assert_eq!(groups.len(), 4);
        let headers: Vec<&str> = groups.iter().map(|(group, _)| group.header()).collect();
        assert_eq!(
            headers,
            [
                "American · Female",
                "American · Male",
                "British · Female",
                "British · Male",
            ]
        );
        let total: usize = groups.iter().map(|(_, ids)| ids.len()).sum();
        assert_eq!(total, VOICES.len());
        let mut seen = Vec::with_capacity(VOICES.len());
        for (group, ids) in &groups {
            assert!(!ids.is_empty(), "{group:?} is empty");
            let names: Vec<String> = ids.iter().map(|id| voice_short_name(id).unwrap()).collect();
            let mut sorted = names.clone();
            sorted.sort_unstable();
            assert_eq!(names, sorted, "{group:?} is not A–Z");
            for id in ids {
                assert_eq!(VoiceGroup::parse(id), Some(*group), "{id}");
                assert!(VOICES.contains(id), "{id}");
                seen.push(*id);
            }
        }
        seen.sort_unstable();
        let mut all = VOICES.to_vec();
        all.sort_unstable();
        assert_eq!(seen, all);
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
                PersianVoice::PocketV2,
                "pocket-v2",
                "Pocket v2 (best quality — recommended)",
                "Pocket v2",
                "pocket-farsi-v2",
                "pocket-farsi-v2.onnx",
            ),
            (
                PersianVoice::Pocket,
                "pocket",
                "Pocket v1",
                "Pocket v1",
                "pocket-farsi-v1",
                "pocket-farsi-v1.onnx",
            ),
            (
                PersianVoice::Amir,
                "amir",
                "Amir (Piper — smaller, faster)",
                "Amir",
                "vits-piper-fa_IR-amir-medium",
                "fa_IR-amir-medium.onnx",
            ),
            (
                PersianVoice::Gyro,
                "gyro",
                "Gyro (Piper — smaller, faster)",
                "Gyro",
                "vits-piper-fa_IR-gyro-medium",
                "fa_IR-gyro-medium.onnx",
            ),
            (
                PersianVoice::RezaIbrahim,
                "reza_ibrahim",
                "Reza Ibrahim (Piper — smaller, faster)",
                "Reza Ibrahim",
                "vits-piper-fa_IR-reza_ibrahim-medium",
                "fa_IR-reza_ibrahim-medium.onnx",
            ),
            (
                PersianVoice::Ganji,
                "ganji",
                "Ganji (Piper — smaller, faster)",
                "Ganji",
                "vits-piper-fa_IR-ganji-medium",
                "fa_IR-ganji-medium.onnx",
            ),
            (
                PersianVoice::GanjiAdabi,
                "ganji_adabi",
                "Ganji Adabi (Piper — smaller, faster)",
                "Ganji Adabi",
                "vits-piper-fa_IR-ganji_adabi-medium",
                "fa_IR-ganji_adabi-medium.onnx",
            ),
        ];
        assert_eq!(PERSIAN_VOICES.len(), expected.len());
        for (voice, id, label, short, dir, file) in expected {
            assert!(PERSIAN_VOICES.contains(&voice), "{id}");
            assert_eq!(voice.id(), id);
            assert_eq!(PersianVoice::parse(id), Some(voice));
            assert_eq!(voice.label(), label);
            assert_eq!(voice.short_label(), short);
            assert_eq!(voice.dir_name(), dir);
            assert_eq!(voice.model_file(), file);
            if !voice.is_piper() {
                assert_eq!(voice.required_files(), [file]);
                let spec = voice
                    .download_spec()
                    .expect("pocket voices have a download spec");
                assert_eq!(spec.url, voice.url());
                let (url, size, sha256) = if voice == PersianVoice::PocketV2 {
                    (
                        "https://huggingface.co/saostad/readly-voices/resolve/main/pocket-farsi-v2/pocket-farsi-v2.onnx",
                        226_378_457,
                        "53a49a5d2fdf94832a0163939c38eb638789ae1f1678dccdd539240dffcf4703",
                    )
                } else {
                    (
                        "https://huggingface.co/saostad/readly-voices/resolve/main/pocket-farsi-v1/pocket-farsi-v1.onnx",
                        226_396_445,
                        "3f3322396a6f8a65e970438f0a58b146e90cd49283cf8c46cef7f3f877cdca30",
                    )
                };
                assert_eq!(voice.url(), url);
                assert_eq!(spec.size, size);
                assert_eq!(spec.sha256, sha256);
            }
            if voice.is_piper() {
                assert_eq!(
                    voice.required_files(),
                    [file, voice.config_file(), "tokens.txt", "espeak-ng-data"]
                );
                assert_eq!(
                    voice.url(),
                    format!(
                        "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/{dir}.tar.bz2"
                    )
                );
                assert_eq!(voice.download_spec(), None);
            }
        }
        assert_eq!(PersianVoice::parse("sara"), None);
        assert_eq!(PersianVoice::parse(""), None);
        assert_eq!(PersianVoice::default(), PersianVoice::PocketV2);
        assert_eq!(PERSIAN_VOICES[0], PersianVoice::PocketV2);
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

    #[test]
    fn phonon_model_definition() {
        let variant = ModelVariant::Phonon;
        assert_eq!(ModelVariant::parse("phonon"), Some(variant));
        assert_eq!(variant.dir_name(), "phonon-31m-en");
        assert_eq!(variant.label(), "Phonon (best quality, ~103 MB)");
        assert_eq!(variant.model_file(), "model.safetensors");
        assert_eq!(
            variant.url(),
            "https://huggingface.co/gradium/phonon-31m-en"
        );
        assert_eq!(
            variant.required_files(),
            [
                "model.safetensors",
                "config.json",
                "tokenizer.json",
                "default-voice.safetensors",
            ]
        );
        // The Kokoro variants ship as one archive: no per-file downloads.
        assert!(ModelVariant::Fast.download_files().is_empty());
        assert!(ModelVariant::Small.download_files().is_empty());
        // Four files with non-zero sizes and 64-hex hashes, matching the
        // required files by name.
        let files = variant.download_files();
        assert_eq!(files.len(), 4);
        let names: Vec<&str> = files.iter().map(|file| file.name).collect();
        assert_eq!(names, variant.required_files());
        for file in files {
            assert!(file.spec.size > 0, "{}", file.name);
            assert_eq!(file.spec.sha256.len(), 64, "{}", file.name);
            assert!(
                file.spec.sha256.bytes().all(|b| b.is_ascii_hexdigit()),
                "{}",
                file.name
            );
            assert!(
                file.spec.url.ends_with(&format!("/{0}", file.name)),
                "{}",
                file.spec.url
            );
        }
    }
}
