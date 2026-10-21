//! Phonon-31m English voice engine (ptts) wrapper.
//!
//! The checkpoint's `default` voice only; synthesis runs on the CPU in f32
//! and speed is applied by resampling afterwards, exactly like Pocket.

use std::path::Path;

use ptts::checkpoint::{Checkpoint, ResolveOptions};
use ptts::preprocess::Lang;
use ptts::synth::{DeviceKind, Quant, SpeechOptions, Synth};

use crate::engine::{EngineError, ModelVariant, clamp_speed};
use crate::pocket::audio::apply_speed;

/// Voice embedding file inside the model dir, registered as `"default"`.
pub(crate) const DEFAULT_VOICE_FILE: &str = "default-voice.safetensors";

/// Loaded Phonon TTS engine.
pub struct PhononEngine {
    tts: Synth,
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
        Ok(Self { tts })
    }

    /// Output sample rate in Hz (24000 for the Phonon checkpoint).
    pub fn sample_rate(&self) -> i32 {
        self.tts.sample_rate() as i32
    }

    /// Synthesize `text` at `speed`, returning mono samples.
    ///
    /// `speed` is clamped to 0.5..=2.0 (non-finite values become 1.0).
    pub fn synthesize(&mut self, text: &str, speed: f32) -> Result<Vec<f32>, EngineError> {
        let text = text.replace('\0', "");
        if text.trim().is_empty() {
            return Err(EngineError::NoAudio(text));
        }
        let speed = clamp_speed(speed);
        let pcm = self
            .tts
            .say_with(&text, &SpeechOptions::default().voice("default"))
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
            .synthesize("Hello from Phonon.", 1.0)
            .expect("synthesize at 1x");
        let fast = tts
            .synthesize("Hello from Phonon.", 2.0)
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
}
