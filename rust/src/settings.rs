//! Persistent user settings (`settings.toml`).

use std::error::Error;
use std::fmt;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::engine::{DEFAULT_VOICE, ModelVariant, PersianVoice};

/// Errors from loading or saving the settings file.
#[derive(Debug)]
pub enum SettingsError {
    /// The file couldn't be read or parsed; defaults are used instead.
    Load { path: PathBuf, message: String },
    /// The file couldn't be written.
    Save { path: PathBuf, message: String },
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SettingsError::Load { path, message } => write!(
                f,
                "Couldn't read settings from {}: {message}. Using defaults until it's fixed.",
                path.display()
            ),
            SettingsError::Save { path, message } => {
                write!(f, "Couldn't save settings to {}: {message}", path.display())
            }
        }
    }
}

impl Error for SettingsError {}

/// Global shortcut bindings, in `global_hotkey::hotkey::HotKey::from_str`
/// syntax (e.g. `"ctrl+alt+Space"`). An empty string disables that shortcut.
/// They apply on the next launch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shortcuts {
    /// Read the clipboard when idle, pause/resume while reading.
    pub read_or_pause: String,
    /// Stop reading.
    pub stop: String,
    /// Jump to the next chunk.
    pub next: String,
    /// Jump to the previous chunk.
    pub previous: String,
}

impl Default for Shortcuts {
    fn default() -> Self {
        Self {
            read_or_pause: "ctrl+alt+Space".to_string(),
            stop: "ctrl+alt+Period".to_string(),
            next: "ctrl+alt+ArrowRight".to_string(),
            previous: "ctrl+alt+ArrowLeft".to_string(),
        }
    }
}

/// User settings. Missing file entries fall back to [`Settings::default`];
/// unknown entries are ignored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Voice name (see `engine::VOICES`).
    pub voice: String,
    /// Speech speed multiplier.
    pub speed: f32,
    /// Model variant.
    pub model: ModelVariant,
    /// Persian voice (serialized as its id, e.g. `"amir"`).
    pub persian_voice: PersianVoice,
    /// Watch the clipboard for new text.
    pub watch_clipboard: bool,
    /// Start reading automatically on new clipboard text.
    pub auto_read: bool,
    /// Minutes of idleness before unloading the model (0 = keep loaded).
    pub unload_after_minutes: u32,
    /// Global shortcut bindings (`[shortcuts]` table).
    pub shortcuts: Shortcuts,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            voice: DEFAULT_VOICE.to_string(),
            speed: 1.0,
            model: ModelVariant::Fast,
            persian_voice: PersianVoice::Amir,
            watch_clipboard: true,
            auto_read: false,
            unload_after_minutes: 5,
            shortcuts: Shortcuts::default(),
        }
    }
}

impl Settings {
    /// Load settings from `path`. A missing file yields defaults with no
    /// error; an unreadable or invalid file yields defaults plus an error.
    /// The file is never overwritten here.
    pub fn load(path: &Path) -> (Settings, Option<SettingsError>) {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == ErrorKind::NotFound => {
                return (Settings::default(), None);
            }
            Err(err) => {
                return (
                    Settings::default(),
                    Some(SettingsError::Load {
                        path: path.to_path_buf(),
                        message: err.to_string(),
                    }),
                );
            }
        };
        match toml::from_str(&text) {
            Ok(settings) => (settings, None),
            Err(err) => (
                Settings::default(),
                Some(SettingsError::Load {
                    path: path.to_path_buf(),
                    message: err.to_string(),
                }),
            ),
        }
    }

    /// Save settings to `path`, creating parent dirs. Writes to `<path>.tmp`
    /// and renames so a crash never leaves a half-written file.
    pub fn save(&self, path: &Path) -> Result<(), SettingsError> {
        let save_err = |err: String| SettingsError::Save {
            path: path.to_path_buf(),
            message: err,
        };
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|err| save_err(err.to_string()))?;
        }
        let text = toml::to_string(self).map_err(|err| save_err(err.to_string()))?;
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        std::fs::write(&tmp, text).map_err(|err| save_err(err.to_string()))?;
        std::fs::rename(&tmp, path).map_err(|err| save_err(err.to_string()))?;
        Ok(())
    }
}

/// Settings file path: `SMART_VOICES_CONFIG` when set, else
/// `dirs::config_dir()/smart-voices/settings.toml`.
pub fn settings_path() -> Option<PathBuf> {
    match std::env::var("SMART_VOICES_CONFIG") {
        Ok(path) if !path.is_empty() => Some(PathBuf::from(path)),
        _ => dirs::config_dir().map(|dir| dir.join("smart-voices").join("settings.toml")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_temp_dir() -> PathBuf {
        let id = TEST_DIR_COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!(
            "smart-voices-settings-test-{}-{id}",
            std::process::id()
        ))
    }

    #[test]
    fn round_trip_save_load() {
        let dir = unique_temp_dir();
        let path = dir.join("sub").join("settings.toml");
        let settings = Settings {
            voice: "bm_lewis".to_string(),
            speed: 1.25,
            model: ModelVariant::Small,
            persian_voice: PersianVoice::Gyro,
            watch_clipboard: false,
            auto_read: true,
            unload_after_minutes: 0,
            shortcuts: Shortcuts {
                read_or_pause: String::new(),
                stop: "ctrl+alt+Q".to_string(),
                ..Shortcuts::default()
            },
        };
        settings.save(&path).unwrap();
        let (loaded, err) = Settings::load(&path);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(err.is_none(), "unexpected error: {err:?}");
        assert_eq!(loaded, settings);
    }

    #[test]
    fn missing_file_yields_defaults_without_error() {
        let dir = unique_temp_dir();
        let (settings, err) = Settings::load(&dir.join("settings.toml"));
        assert!(err.is_none(), "unexpected error: {err:?}");
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn invalid_toml_yields_defaults_with_error() {
        let dir = unique_temp_dir();
        let path = dir.join("settings.toml");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "voice = [unclosed\n").unwrap();
        let before = std::fs::read_to_string(&path).unwrap();
        let (settings, err) = Settings::load(&path);
        let after = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(settings, Settings::default());
        let err = err.expect("expected an error for invalid TOML");
        let msg = err.to_string();
        assert!(msg.contains(&path.to_string_lossy().into_owned()), "{msg}");
        assert!(msg.contains("Using defaults until it's fixed"), "{msg}");
        assert_eq!(
            before, after,
            "broken settings file must not be overwritten"
        );
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let dir = unique_temp_dir();
        let path = dir.join("settings.toml");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            &path,
            "voice = \"bm_lewis\"\nfuture_option = 42\n[unknown_table]\nkey = true\n",
        )
        .unwrap();
        let (settings, err) = Settings::load(&path);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(err.is_none(), "unexpected error: {err:?}");
        assert_eq!(settings.voice, "bm_lewis");
        assert_eq!(settings.speed, 1.0);
    }

    #[test]
    fn partial_file_fills_defaults() {
        let dir = unique_temp_dir();
        let path = dir.join("settings.toml");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "model = \"small\"\nauto_read = true\n").unwrap();
        let (settings, err) = Settings::load(&path);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(err.is_none(), "unexpected error: {err:?}");
        assert_eq!(
            settings,
            Settings {
                model: ModelVariant::Small,
                auto_read: true,
                ..Settings::default()
            }
        );
    }

    #[test]
    fn old_file_without_shortcuts_loads_defaults() {
        let dir = unique_temp_dir();
        let path = dir.join("settings.toml");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "voice = \"bm_lewis\"\nunload_after_minutes = 0\n").unwrap();
        let (settings, err) = Settings::load(&path);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(err.is_none(), "unexpected error: {err:?}");
        assert_eq!(settings.voice, "bm_lewis");
        assert_eq!(settings.shortcuts, Shortcuts::default());
        assert_eq!(settings.persian_voice, PersianVoice::Amir);
    }

    #[test]
    fn persian_voice_serializes_as_its_id() {
        let dir = unique_temp_dir();
        let path = dir.join("settings.toml");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "persian_voice = \"reza_ibrahim\"\n").unwrap();
        let (settings, err) = Settings::load(&path);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(err.is_none(), "unexpected error: {err:?}");
        assert_eq!(settings.persian_voice, PersianVoice::RezaIbrahim);
    }

    #[test]
    fn partial_shortcuts_fill_defaults() {
        let dir = unique_temp_dir();
        let path = dir.join("settings.toml");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "[shortcuts]\nstop = \"\"\n").unwrap();
        let (settings, err) = Settings::load(&path);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(err.is_none(), "unexpected error: {err:?}");
        assert_eq!(
            settings.shortcuts,
            Shortcuts {
                stop: String::new(),
                ..Shortcuts::default()
            }
        );
    }

    #[test]
    fn env_override_selects_settings_path() {
        let dir = unique_temp_dir();
        let path = dir.join("custom.toml");
        let key = "SMART_VOICES_CONFIG";
        let previous = std::env::var(key).ok();
        // SAFETY: no other test in this binary touches process env vars.
        unsafe {
            std::env::set_var(key, &path);
        }
        let resolved = settings_path();
        // SAFETY: same as above; restores the pre-test state.
        unsafe {
            match previous {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
        assert_eq!(resolved, Some(path));
    }
}
