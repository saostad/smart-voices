//! Pocket TTS runtime settings (`[pocket]` table).

use serde::{Deserialize, Serialize};

/// Runtime settings (`[pocket]` table).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PocketSettings {
    /// Highest token budget per chunk.
    pub max_tokens: usize,
    /// Lowest token budget per chunk.
    pub min_tokens: usize,
    /// Sampling temperature for generation.
    pub temperature: f32,
    /// Sampler decode steps; 0 uses the model's own value.
    pub decode_steps: u32,
    /// EOS logit threshold above which generation ends.
    pub eos_threshold: f32,
    /// Frames to keep generating after EOS.
    pub frames_after_eos: usize,
    /// Characters per window when pronouncing a sentence with G2P.
    pub g2p_window_chars: usize,
}

impl Default for PocketSettings {
    fn default() -> Self {
        Self {
            max_tokens: 50,
            min_tokens: 8,
            temperature: 0.3,
            decode_steps: 0,
            eos_threshold: -2.0,
            frames_after_eos: 0,
            g2p_window_chars: 120,
        }
    }
}

impl PocketSettings {
    pub fn validate(&self) -> Result<(), String> {
        if !(8..=50).contains(&self.max_tokens) {
            return Err("pocket.max_tokens must be between 8 and 50".into());
        }
        if self.min_tokens < 1 || self.min_tokens > self.max_tokens {
            return Err(format!(
                "pocket.min_tokens must be between 1 and {}",
                self.max_tokens
            ));
        }
        if !self.temperature.is_finite() || !(0.0..=1.5).contains(&self.temperature) {
            return Err("pocket.temperature must be between 0 and 1.5".into());
        }
        if !(0..=4).contains(&self.decode_steps) {
            return Err("pocket.decode_steps must be between 0 and 4".into());
        }
        if !self.eos_threshold.is_finite() || !(-10.0..=10.0).contains(&self.eos_threshold) {
            return Err("pocket.eos_threshold must be between -10 and 10".into());
        }
        if !(0..=10).contains(&self.frames_after_eos) {
            return Err("pocket.frames_after_eos must be between 0 and 10".into());
        }
        if !(40..=1000).contains(&self.g2p_window_chars) {
            return Err("pocket.g2p_window_chars must be between 40 and 1000".into());
        }
        Ok(())
    }
}

/// Current Pocket settings: the scoped invocation's, else the settings file.
/// A missing or invalid file falls back to the defaults (logged; the app
/// already warned about the file at startup).
pub fn current_settings() -> PocketSettings {
    if let Some(settings) = crate::settings::invocation_settings() {
        return settings.pocket;
    }
    let Some(path) = crate::settings::settings_path() else {
        eprintln!(
            "warn: fallback: true, attempted=settings_file, reason=settings_directory_unavailable, taken=default_pocket_settings"
        );
        return PocketSettings::default();
    };
    let (settings, error) = crate::settings::Settings::load(&path);
    if let Some(error) = error {
        eprintln!(
            "warn: fallback: true, attempted=settings_file, reason={error}, taken=default_pocket_settings"
        );
    }
    settings.pocket
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_validate_with_longer_chunks() {
        let settings = PocketSettings::default();
        assert_eq!(settings.max_tokens, 50);
        assert_eq!(settings.min_tokens, 8);
        assert_eq!(settings.temperature, 0.3);
        assert_eq!(settings.decode_steps, 0);
        assert_eq!(settings.eos_threshold, -2.0);
        assert_eq!(settings.frames_after_eos, 0);
        assert_eq!(settings.g2p_window_chars, 120);
        assert!(settings.validate().is_ok());
    }

    #[test]
    fn out_of_range_values_fail_with_their_key() {
        let cases: Vec<(PocketSettings, &str)> = vec![
            (
                PocketSettings {
                    max_tokens: 7,
                    ..Default::default()
                },
                "pocket.max_tokens",
            ),
            (
                PocketSettings {
                    max_tokens: 51,
                    ..Default::default()
                },
                "pocket.max_tokens",
            ),
            (
                PocketSettings {
                    min_tokens: 0,
                    ..Default::default()
                },
                "pocket.min_tokens",
            ),
            (
                PocketSettings {
                    temperature: 1.6,
                    ..Default::default()
                },
                "pocket.temperature",
            ),
            (
                PocketSettings {
                    temperature: f32::NAN,
                    ..Default::default()
                },
                "pocket.temperature",
            ),
            (
                PocketSettings {
                    decode_steps: 5,
                    ..Default::default()
                },
                "pocket.decode_steps",
            ),
            (
                PocketSettings {
                    eos_threshold: -10.1,
                    ..Default::default()
                },
                "pocket.eos_threshold",
            ),
            (
                PocketSettings {
                    eos_threshold: f32::INFINITY,
                    ..Default::default()
                },
                "pocket.eos_threshold",
            ),
            (
                PocketSettings {
                    frames_after_eos: 11,
                    ..Default::default()
                },
                "pocket.frames_after_eos",
            ),
            (
                PocketSettings {
                    g2p_window_chars: 39,
                    ..Default::default()
                },
                "pocket.g2p_window_chars",
            ),
            (
                PocketSettings {
                    g2p_window_chars: 1001,
                    ..Default::default()
                },
                "pocket.g2p_window_chars",
            ),
        ];
        for (settings, key) in cases {
            let message = settings.validate().unwrap_err();
            assert!(message.contains(key), "{message}");
        }
    }

    #[test]
    fn min_tokens_above_max_tokens_fails() {
        let settings = PocketSettings {
            max_tokens: 18,
            min_tokens: 19,
            ..Default::default()
        };
        let message = settings.validate().unwrap_err();
        assert!(message.contains("pocket.min_tokens"), "{message}");
    }
}
