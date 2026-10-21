//! Phonon-31m English voice engine (ptts) wrapper.
//!
//! The checkpoint's `default` voice plus the English voices of the Gradium
//! voice catalog ([`PHONON_VOICES`], downloaded together as
//! [`VOICES_DIR`]); catalog voices are registered on first use. Synthesis
//! runs on the CPU in f32 and speed is applied by resampling afterwards,
//! exactly like Pocket.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use ptts::checkpoint::{Checkpoint, ResolveOptions};
use ptts::preprocess::Lang;
use ptts::synth::{DeviceKind, Quant, SpeechOptions, Synth};

use crate::engine::{DownloadFile, DownloadSpec, EngineError, ModelVariant, clamp_speed};
use crate::pocket::audio::apply_speed;

/// Voice embedding file inside the model dir, registered as `"default"`.
pub(crate) const DEFAULT_VOICE_FILE: &str = "default-voice.safetensors";

/// Id of the checkpoint's own voice (the default Phonon voice).
pub const DEFAULT_PHONON_VOICE: &str = "default";

/// Dir under the models root holding the catalog voice files
/// (`<id>.safetensors` each).
pub const VOICES_DIR: &str = "phonon-voices-en";

/// Menu group of a catalog voice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhononVoiceGroup {
    AmericanFemale,
    AmericanMale,
    BritishFemale,
    BritishMale,
    Characters,
}

impl PhononVoiceGroup {
    /// All groups in menu order.
    pub const ALL: [Self; 5] = [
        Self::AmericanFemale,
        Self::AmericanMale,
        Self::BritishFemale,
        Self::BritishMale,
        Self::Characters,
    ];

    /// Disabled-header label for the `English Voice` submenu.
    pub fn header(self) -> &'static str {
        match self {
            Self::AmericanFemale => "American · Female",
            Self::AmericanMale => "American · Male",
            Self::BritishFemale => "British · Female",
            Self::BritishMale => "British · Male",
            Self::Characters => "Characters",
        }
    }
}

/// One English voice of the Gradium Phonon voice catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhononVoice {
    /// Catalog voice id (also the file stem); stored in settings.
    pub id: &'static str,
    /// Display name.
    pub name: &'static str,
    pub group: PhononVoiceGroup,
    /// The voice file inside [`VOICES_DIR`].
    pub file: DownloadFile,
}

/// Builds a [`PhononVoice`] pinned to a `gradium/phonon-voices` commit.
macro_rules! phonon_voice {
    ($group:ident, $name:literal, $id:literal, $sha256:literal) => {
        PhononVoice {
            id: $id,
            name: $name,
            group: PhononVoiceGroup::$group,
            file: DownloadFile {
                name: concat!($id, ".safetensors"),
                spec: DownloadSpec {
                    url: concat!(
                        "https://huggingface.co/gradium/phonon-voices/resolve/0ee17205f254b5abc43b266c8a0f215565c352f7/voices/",
                        $id,
                        ".safetensors"
                    ),
                    size: 256_136,
                    sha256: $sha256,
                },
            },
        }
    };
}

/// The catalog's English voices, by group in menu order, A–Z within each.
pub const PHONON_VOICES: [PhononVoice; 37] = [
    phonon_voice!(
        AmericanFemale,
        "Delia",
        "nGzMHMPAWGaJFHOo",
        "a0c3dcb91b2247eca56536be9ae8e8becc1c807b1136c5fe101552d6a155b24a"
    ),
    phonon_voice!(
        AmericanFemale,
        "Everly",
        "vjFyp2BcCyTlUmYb",
        "958ea94da14f46dd48c7f64adac11ed2ad2aa45f7ccd165dfebab5272664402e"
    ),
    phonon_voice!(
        AmericanFemale,
        "Hadley",
        "bXwrkHhURL1VFJdG",
        "a879938aa9a2df0a8220bdadef88137929f71e887bd91e7db205bd5fa11661d2"
    ),
    phonon_voice!(
        AmericanFemale,
        "Harper",
        "4SZHfMpw-p46Ywgs",
        "6b3da6e2d796a20c51af892c482bd6509fb88267a221aff83c870843dee2a123"
    ),
    phonon_voice!(
        AmericanFemale,
        "Juniper",
        "NIWIOv5vTi5eRMj8",
        "444378eb06b7c492b86e8fc7d13e4db1a3c0cf3e68891a960bf6f0a41a13ef35"
    ),
    phonon_voice!(
        AmericanFemale,
        "Zoey",
        "NbpkqMVS3CJeq2j8",
        "97c7f8c253413574406ab80dd23f49c3ebb2517163b00a9bf86b9c4cf43e7e23"
    ),
    phonon_voice!(
        AmericanMale,
        "Easton",
        "LMDIYE0Oa377Qh6Z",
        "45c582673cb20edb04ddcca2b5a8865ddfb0a82599efadb269021e87d20fbdd4"
    ),
    phonon_voice!(
        AmericanMale,
        "Micah",
        "40QrMVgAxFiUnP9f",
        "0de1098d1051686a138d068e83ff09deb5053328bc74ec9541a1fdb90bfc24da"
    ),
    phonon_voice!(
        AmericanMale,
        "Miles",
        "d9XCyhjo4JhkHF1d",
        "eab20e4f159b48cfbc01b1885804e24f681f80e7a64f909182763a4e3361bee8"
    ),
    phonon_voice!(
        AmericanMale,
        "Preston",
        "qIxlZ7Ei8KvWlyQH",
        "6e04b84d3ae8751b22bca0a4f63936f9cb648f4f2182b2d0788071ea1c0b68a1"
    ),
    phonon_voice!(
        AmericanMale,
        "Russell",
        "_6Aslh2DxfmnRLmP",
        "beae9e4030efce0a935fcb0f807dc0784d598bcaa87ad4aacb047436b1eaa411"
    ),
    phonon_voice!(
        AmericanMale,
        "Trevor",
        "0zLApsZfVLLNaEGl",
        "dc1341c8937af5534e6bbcc5c0fc5e7c10312229b59061228074f93683fe2261"
    ),
    phonon_voice!(
        BritishFemale,
        "Bryony",
        "gfP44RlHwr2kEwk7",
        "bcc207a73b7885e45b49ab633d3377a10158292e3a7cef9cd6764ad107f1cb9b"
    ),
    phonon_voice!(
        BritishFemale,
        "Elodie-Rose",
        "y9bQqIDnyxwqK01t",
        "21eade9d83b2e66b9364e1b6cd999a3a6704c000a71f27f34e05ace5cdfd58bb"
    ),
    phonon_voice!(
        BritishFemale,
        "Freya",
        "GgfEkEJtxZR7gnpy",
        "eb3cd47418349e618408c16bea7153988b3ad04b41a6033039dd474a9a4dcd23"
    ),
    phonon_voice!(
        BritishFemale,
        "Imogen",
        "gDT1nz7Ie36ZhL-C",
        "1980ab3c35fa98a25b5f3b5e643fd852e14ca8a12a4a26b06aaf1abf2a8c68b1"
    ),
    phonon_voice!(
        BritishFemale,
        "Lottie",
        "m9MxpY6IUGXtWp76",
        "a906ebf17e43fded12b3d6bd077abb2df33bc86eff51c1beeb82e5d416527db3"
    ),
    phonon_voice!(
        BritishFemale,
        "Maeve",
        "6PWnV0Nq4wu7RVBT",
        "56494ee67d533ad227ff0dfd0b1c13ff5aa91f1ad6e596e66c2986ba3005e844"
    ),
    phonon_voice!(
        BritishFemale,
        "Pippa",
        "uem82D50GRv2Dwma",
        "d41a8f01d6e33450f1ce062d332914da10beee99e585dab531750706f187c48a"
    ),
    phonon_voice!(
        BritishMale,
        "Alfie",
        "X6NceqF285Gr04gb",
        "4de75497fff3030168e8aa6dca23ffeb332b1bc8c25d0bf3507ceadac953ae9c"
    ),
    phonon_voice!(
        BritishMale,
        "Edwin",
        "zUCxzrOrugTSis03",
        "ff1d0735b8b45710462b1d1f846da040f970c6eb62c7c2f36dbfb8e5bde74f1e"
    ),
    phonon_voice!(
        BritishMale,
        "Hugh",
        "YyKcxPG7iIgS3jsK",
        "3169d74b47a54d1eb15e4c243f56393abe9fe90c7e1c308ecf96ee67618b152a"
    ),
    phonon_voice!(
        BritishMale,
        "Spencer",
        "JOW0vchyswkQUMDS",
        "cc9ac1107b0ffce282374a19beb7bfa95847724e6b7afbac991b4ae4277d5e6d"
    ),
    phonon_voice!(
        BritishMale,
        "Teddy",
        "Zn7g6NxLqXlJb7Qi",
        "baea0ea9506f3237a5ebfc61afb7c72dbd189c5603b199ef42fbb00dbda379e0"
    ),
    phonon_voice!(
        BritishMale,
        "Toby",
        "dME3IWyZBvmh1n1q",
        "09ab315c7f4b72b4d71e66cb756184cedfb1f14d222d096e115bf95bfe7b29dc"
    ),
    phonon_voice!(
        Characters,
        "Azgorath",
        "dGyGDKo2osj7h3s7",
        "83900e020c14946426614c6553c5a138ba6277e1aaa69832223bd0f0f8b73893"
    ),
    phonon_voice!(
        Characters,
        "Captain Silas Blackwater",
        "xPDpsV3F592srkjA",
        "a09f615b0084980ebc4076d71550bf73ccd8d4df076bbbee4a9beb523a1379d9"
    ),
    phonon_voice!(
        Characters,
        "Fairy Godmother",
        "QXNr0SvkukLB4wPq",
        "f390c09f5e279c0f2171995a534b92f5ca5c83443906879da68fc27445f89d89"
    ),
    phonon_voice!(
        Characters,
        "Fennick the Young Elf",
        "cXoIXJ2llS8S0WOW",
        "a05944e4c69d61d10d66f689175ba0b081930df2d77b22f04d00ac40d3d69ecb"
    ),
    phonon_voice!(
        Characters,
        "Grub the Ogre",
        "cYtsRSxV5q9IMehn",
        "b30fa847686ec1eb6be8c5bea4b6a5c5076ea66c3ff053cf98210a155d8aad1e"
    ),
    phonon_voice!(
        Characters,
        "Master Aldric",
        "gSgG5Tor0GPEnqzC",
        "a83430cc022b85c83753f859f9e44f406651646dab994fced8ef7a9a88005971"
    ),
    phonon_voice!(
        Characters,
        "Morwenna of the Deep",
        "nOykTmcH9ydsGGIc",
        "2ff2db91e70648623df8d42c0dcd7c336b2f14cb4362d602a20f292a4d862154"
    ),
    phonon_voice!(
        Characters,
        "Old Mortlake",
        "lX3LavMNdlIbgEHH",
        "d3f9429205e5906dcd3c78edaf7c9d5148c7e389e34c82c86f72f154adb5d923"
    ),
    phonon_voice!(
        Characters,
        "Sir Bramwell",
        "y2eiKxtQo2B67deU",
        "b3cea6789aeb58822af16ba08c4e5105d7febb840dcb67b47f426c19d1a8f209"
    ),
    phonon_voice!(
        Characters,
        "The Snow Queen",
        "NIWoEurJHMJjyUTe",
        "bc8a8fcb95bb4f32d2d4376aa69d81b186b3ba2f8e9b6cb6a403b2b194590e85"
    ),
    phonon_voice!(
        Characters,
        "The Thing Under the Stairs",
        "GXaSqMEmOg12OIYb",
        "bfa37bac98aefbb9de80f792c47d01a0f9e3b94e73776169040c249c8b722759"
    ),
    phonon_voice!(
        Characters,
        "Vyrmathrax the Ancient",
        "xO8xQo3BrB8FuI6J",
        "63ab4b15427844a804480a1603774262e8903dee33cb279e2749860dbfaedd10"
    ),
];

/// Look up a catalog voice by id, or by name in any case (`"zoey"`).
pub fn find_voice(query: &str) -> Option<&'static PhononVoice> {
    PHONON_VOICES
        .iter()
        .find(|voice| voice.id == query || voice.name.eq_ignore_ascii_case(query))
}

/// True for [`DEFAULT_PHONON_VOICE`] or a catalog voice id.
pub fn is_known_voice(id: &str) -> bool {
    id == DEFAULT_PHONON_VOICE || PHONON_VOICES.iter().any(|voice| voice.id == id)
}

/// Loaded Phonon TTS engine.
pub struct PhononEngine {
    tts: Synth,
    /// Where the catalog voice files live (`models_root/phonon-voices-en`).
    voices_dir: PathBuf,
    /// Catalog voices already registered with `tts`.
    registered: HashSet<&'static str>,
}

impl PhononEngine {
    /// Load the Phonon voice model from `models_root/phonon-31m-en`.
    pub fn load(models_root: &Path) -> Result<Self, EngineError> {
        let dir = models_root.join(ModelVariant::Phonon.dir_name());
        // The model file is checked first so a bare directory reports the
        // most actionable missing path.
        for name in ModelVariant::Phonon.required_files() {
            let path = dir.join(name);
            if !path.exists() {
                return Err(EngineError::MissingModel(path));
            }
        }

        let checkpoint = Checkpoint::resolve(
            &dir,
            ResolveOptions {
                quant: Quant::F32,
                weights: Some("model.safetensors"),
            },
        )
        .map_err(|err| EngineError::PhononLoadFailed(dir.clone(), err.to_string()))?;
        let mut tts = checkpoint
            .builder(Lang::En)
            .device(DeviceKind::Cpu)
            .build()
            .map_err(|err| EngineError::PhononLoadFailed(dir.clone(), err.to_string()))?;
        // Load the default voice loudly: `register_voices` only warns (and this
        // app installs no tracing subscriber), which would leave synthesis
        // running unconditioned in a random-sounding voice.
        tts.add_voice_file("default", &dir.join(DEFAULT_VOICE_FILE))
            .map_err(|err| EngineError::PhononLoadFailed(dir.clone(), err.to_string()))?;
        Ok(Self {
            tts,
            voices_dir: models_root.join(VOICES_DIR),
            registered: HashSet::new(),
        })
    }

    /// Register catalog voice `id` on first use (loudly, like the default
    /// voice). Returns the name to synthesize with.
    fn voice_name(&mut self, id: &str) -> Result<&'static str, EngineError> {
        if id == DEFAULT_PHONON_VOICE {
            return Ok(DEFAULT_PHONON_VOICE);
        }
        let voice = PHONON_VOICES
            .iter()
            .find(|voice| voice.id == id)
            .ok_or_else(|| EngineError::PhononFailed(format!("unknown Phonon voice '{id}'")))?;
        if !self.registered.contains(voice.id) {
            let path = self.voices_dir.join(voice.file.name);
            if !path.exists() {
                return Err(EngineError::MissingModel(path));
            }
            self.tts.add_voice_file(voice.id, &path).map_err(|err| {
                EngineError::PhononFailed(format!(
                    "couldn't load the {} voice from {}: {err}",
                    voice.name,
                    path.display()
                ))
            })?;
            self.registered.insert(voice.id);
        }
        Ok(voice.id)
    }

    /// Output sample rate in Hz (24000 for the Phonon checkpoint).
    pub fn sample_rate(&self) -> i32 {
        self.tts.sample_rate() as i32
    }

    /// Synthesize `text` in `voice` (an id from [`is_known_voice`]) at
    /// `speed`, returning mono samples.
    ///
    /// `speed` is clamped to 0.5..=2.0 (non-finite values become 1.0).
    pub fn synthesize(
        &mut self,
        text: &str,
        voice: &str,
        speed: f32,
    ) -> Result<Vec<f32>, EngineError> {
        let text = text.replace('\0', "");
        if text.trim().is_empty() {
            return Err(EngineError::NoAudio(text));
        }
        let speed = clamp_speed(speed);
        let voice = self.voice_name(voice)?;
        let pcm = self
            .tts
            .say_with(&text, &SpeechOptions::default().voice(voice))
            .map_err(|err| EngineError::PhononFailed(err.to_string()))?;
        if pcm.is_empty() {
            return Err(EngineError::NoAudio(text));
        }
        Ok(apply_speed(&pcm, speed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_temp_dir() -> PathBuf {
        let id = TEST_DIR_COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!(
            "smart-voices-phonon-test-{}-{id}",
            std::process::id()
        ))
    }

    #[test]
    fn load_on_missing_dir_reports_missing_model_file() {
        let dir = unique_temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let result = PhononEngine::load(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
        match result {
            Err(EngineError::MissingModel(path)) => {
                assert_eq!(
                    path,
                    dir.join(ModelVariant::Phonon.dir_name())
                        .join("model.safetensors")
                );
            }
            Ok(_) => panic!("expected MissingModel, got Ok"),
            Err(other) => panic!("expected MissingModel, got {other}"),
        }
    }

    #[test]
    fn phonon_error_messages_keep_the_real_text() {
        let err = EngineError::PhononLoadFailed(
            PathBuf::from("/models/phonon-31m-en"),
            "no weights in \"/models/phonon-31m-en\"".to_string(),
        );
        assert_eq!(
            err.to_string(),
            "Couldn't load the Phonon voice model from /models/phonon-31m-en: no weights in \"/models/phonon-31m-en\". The files may be damaged; delete the folder and run `smart-voices download --model phonon` again."
        );
        let err = EngineError::PhononFailed("the voice prompt alone is 5 frames".to_string());
        assert_eq!(
            err.to_string(),
            "The Phonon voice failed: the voice prompt alone is 5 frames"
        );
    }

    /// Against the real model: set `SMART_VOICES_PHONON_DIR` to a models root
    /// holding `phonon-31m-en/`.
    #[test]
    #[ignore]
    fn synthesize_hello_from_phonon() {
        let root = PathBuf::from(
            std::env::var("SMART_VOICES_PHONON_DIR")
                .expect("set SMART_VOICES_PHONON_DIR to a models root holding phonon-31m-en/"),
        );
        let mut tts = PhononEngine::load(&root).expect("load the Phonon model");
        assert_eq!(tts.sample_rate(), 24000);
        let slow = tts
            .synthesize("Hello from Phonon.", DEFAULT_PHONON_VOICE, 1.0)
            .expect("synthesize at 1x");
        let fast = tts
            .synthesize("Hello from Phonon.", DEFAULT_PHONON_VOICE, 2.0)
            .expect("synthesize at 2x");
        assert!(!slow.is_empty());
        assert!(!fast.is_empty());
        assert!(
            fast.len() < slow.len(),
            "2x gave {} samples, 1x gave {}",
            fast.len(),
            slow.len()
        );

        // A damaged voice file must fail the load loudly, never fall back to
        // an unconditioned voice.
        let scratch = unique_temp_dir();
        let source = root.join(ModelVariant::Phonon.dir_name());
        let damaged = scratch.join(ModelVariant::Phonon.dir_name());
        std::fs::create_dir_all(&damaged).unwrap();
        for name in ModelVariant::Phonon.required_files() {
            std::fs::copy(source.join(name), damaged.join(name)).unwrap();
        }
        std::fs::write(damaged.join(DEFAULT_VOICE_FILE), b"not a voice").unwrap();
        match PhononEngine::load(&scratch) {
            Err(EngineError::PhononLoadFailed(_, _)) => {}
            Ok(_) => panic!("expected PhononLoadFailed, got Ok"),
            Err(other) => panic!("expected PhononLoadFailed, got {other}"),
        }
        std::fs::remove_dir_all(&scratch).unwrap();
    }

    #[test]
    fn catalog_holds_37_unique_voices_with_pinned_urls() {
        use std::collections::HashSet;

        assert_eq!(PHONON_VOICES.len(), 37);
        let ids: HashSet<&str> = PHONON_VOICES.iter().map(|voice| voice.id).collect();
        assert_eq!(ids.len(), 37, "catalog voice ids must be unique");
        for voice in PHONON_VOICES {
            assert_eq!(voice.file.name, format!("{}.safetensors", voice.id));
            let suffix = format!("/voices/{}.safetensors", voice.id);
            assert!(
                voice.file.spec.url.ends_with(&suffix),
                "URL for {} doesn't end with {suffix}: {}",
                voice.id,
                voice.file.spec.url
            );
        }
    }

    #[test]
    fn find_voice_matches_name_in_any_case() {
        let zoey = find_voice("zoey").expect("find Zoey by lowercase name");
        assert_eq!(zoey.name, "Zoey");
        assert_eq!(zoey.id, "NbpkqMVS3CJeq2j8");
        assert_eq!(
            find_voice("NbpkqMVS3CJeq2j8").map(|voice| voice.id),
            Some(zoey.id)
        );
        assert_eq!(find_voice("ZOEY").map(|voice| voice.id), Some(zoey.id));
        assert!(find_voice("no such voice").is_none());
        assert!(is_known_voice(DEFAULT_PHONON_VOICE));
        assert!(is_known_voice("NbpkqMVS3CJeq2j8"));
        assert!(!is_known_voice("zoey"));
        assert!(!is_known_voice("no such voice"));
    }

    /// Against the real model: set `SMART_VOICES_PHONON_DIR` to a models root
    /// holding `phonon-31m-en/` without the catalog voices installed.
    #[test]
    #[ignore]
    fn synthesize_missing_catalog_voice_reports_its_path() {
        let root = PathBuf::from(
            std::env::var("SMART_VOICES_PHONON_DIR")
                .expect("set SMART_VOICES_PHONON_DIR to a models root holding phonon-31m-en/"),
        );
        let expected = root.join(VOICES_DIR).join("NbpkqMVS3CJeq2j8.safetensors");
        if expected.exists() {
            eprintln!("skipped: the catalog voices are installed in {root:?}");
            return;
        }
        let mut tts = PhononEngine::load(&root).expect("load the Phonon model");
        match tts.synthesize("Hello from Phonon.", "NbpkqMVS3CJeq2j8", 1.0) {
            Err(EngineError::MissingModel(path)) => assert_eq!(path, expected),
            Ok(_) => panic!("expected MissingModel, got Ok"),
            Err(other) => panic!("expected MissingModel, got {other}"),
        }
    }
}
