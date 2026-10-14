//! Pronunciation dictionary (port of `PronunciationDictionary.swift`).
//!
//! Applies written→spoken rules to text right before synthesis: longest match
//! first, output never reprocessed, whole-word boundary checks via
//! `char::is_alphanumeric`. Lowercased rules are precomputed once at
//! construction, not per position. Stored as `[[entry]]` tables in
//! `pronunciations.toml`, next to the settings file.

use std::error::Error;
use std::fmt;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::settings::settings_path;

/// Errors from loading or saving the pronunciations file.
#[derive(Debug)]
pub enum PronounceError {
    /// The file couldn't be read or parsed; the built-in list is used instead.
    Load { path: PathBuf, message: String },
    /// The file couldn't be written.
    Save { path: PathBuf, message: String },
}

impl fmt::Display for PronounceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PronounceError::Load { path, message } => write!(
                f,
                "Couldn't read pronunciations from {}: {message}. Using the built-in list until it's fixed.",
                path.display()
            ),
            PronounceError::Save { path, message } => {
                write!(
                    f,
                    "Couldn't save pronunciations to {}: {message}",
                    path.display()
                )
            }
        }
    }
}

impl Error for PronounceError {}

fn default_whole_word() -> bool {
    true
}

/// One pronunciation rule: how a written word should be spoken.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PronunciationEntry {
    pub written: String,
    pub spoken: String,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default = "default_whole_word")]
    pub whole_word: bool,
}

impl PronunciationEntry {
    /// Case-insensitive whole-word rule.
    pub fn new(written: &str, spoken: &str) -> Self {
        Self {
            written: written.to_string(),
            spoken: spoken.to_string(),
            case_sensitive: false,
            whole_word: true,
        }
    }

    /// Seed rules for common acronyms and jargon.
    pub fn defaults() -> Vec<Self> {
        vec![
            Self::new("SQL", "sequel"),
            Self::new("macOS", "mac O S"),
            Self::new("iOS", "eye O S"),
            Self::new("GitHub", "git hub"),
            Self::new("LLM", "L L M"),
            Self::new("TTS", "T T S"),
            Self::new("API", "A P I"),
            Self::new("UI", "U I"),
            Self::new("JSON", "jason"),
            Self::new("Kubernetes", "koo ber net ees"),
        ]
    }
}

/// One rule, precomputed for matching: lowercased form and char length are
/// computed once at construction, not per text position.
#[derive(Debug, Clone)]
struct Rule {
    written: String,
    lowered: String,
    spoken: String,
    case_sensitive: bool,
    whole_word: bool,
    len: usize,
}

/// Applies pronunciation rules to text right before synthesis.
#[derive(Debug, Clone)]
pub struct Pronunciations {
    /// Rules longest-match-first (stable: ties keep insertion order).
    rules: Vec<Rule>,
}

impl Pronunciations {
    /// Build from entries: empty `written` rules are dropped, the rest are
    /// sorted longest-first with lowercased forms precomputed.
    pub fn new(entries: Vec<PronunciationEntry>) -> Self {
        let mut rules: Vec<Rule> = entries
            .into_iter()
            .filter(|entry| !entry.written.is_empty())
            .map(|entry| Rule {
                len: entry.written.chars().count(),
                lowered: entry.written.to_lowercase(),
                written: entry.written,
                spoken: entry.spoken,
                case_sensitive: entry.case_sensitive,
                whole_word: entry.whole_word,
            })
            .collect();
        rules.sort_by_key(|rule| std::cmp::Reverse(rule.len));
        Self { rules }
    }

    /// Dictionary with the built-in seed rules.
    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(PronunciationEntry::defaults())
    }

    /// Rules in longest-match-first order.
    #[must_use]
    pub fn entries(&self) -> Vec<PronunciationEntry> {
        self.rules
            .iter()
            .map(|rule| PronunciationEntry {
                written: rule.written.clone(),
                spoken: rule.spoken.clone(),
                case_sensitive: rule.case_sensitive,
                whole_word: rule.whole_word,
            })
            .collect()
    }

    /// Apply the rules to `text`. Output is never reprocessed.
    #[must_use]
    pub fn apply(&self, text: &str) -> String {
        if self.rules.is_empty() {
            return text.to_string();
        }
        let chars: Vec<char> = text.chars().collect();
        let mut result = String::with_capacity(text.len());
        let mut index = 0;
        while index < chars.len() {
            let mut matched: Option<&Rule> = None;
            for rule in &self.rules {
                if rule_matches(rule, &chars, index) {
                    matched = Some(rule);
                    break;
                }
            }
            match matched {
                Some(rule) => {
                    result.push_str(&rule.spoken);
                    index += rule.len;
                }
                None => {
                    result.push(chars[index]);
                    index += 1;
                }
            }
        }
        result
    }

    /// Load from `path`. A missing file is seeded with the built-in defaults
    /// (written to `path` so the user has something to edit) and used with no
    /// error. An unreadable or invalid file yields the defaults plus an error;
    /// the file is never overwritten in that case.
    pub fn load(path: &Path) -> (Self, Option<PronounceError>) {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == ErrorKind::NotFound => {
                let defaults = Self::with_defaults();
                return match defaults.save(path) {
                    Ok(()) => (defaults, None),
                    Err(err) => (Self::with_defaults(), Some(err)),
                };
            }
            Err(err) => {
                return (
                    Self::with_defaults(),
                    Some(PronounceError::Load {
                        path: path.to_path_buf(),
                        message: err.to_string(),
                    }),
                );
            }
        };
        match toml::from_str::<PronunciationsFile>(&text) {
            Ok(file) => (Self::new(file.entry), None),
            Err(err) => (
                Self::with_defaults(),
                Some(PronounceError::Load {
                    path: path.to_path_buf(),
                    message: err.to_string(),
                }),
            ),
        }
    }

    /// Save to `path`, creating parent dirs. Writes to `<path>.tmp` and
    /// renames so a crash never leaves a half-written file.
    pub fn save(&self, path: &Path) -> Result<(), PronounceError> {
        let save_err = |err: String| PronounceError::Save {
            path: path.to_path_buf(),
            message: err,
        };
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|err| save_err(err.to_string()))?;
        }
        let file = PronunciationsFile {
            entry: self.entries(),
        };
        let text = toml::to_string(&file).map_err(|err| save_err(err.to_string()))?;
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        std::fs::write(&tmp, text).map_err(|err| save_err(err.to_string()))?;
        std::fs::rename(&tmp, path).map_err(|err| save_err(err.to_string()))?;
        Ok(())
    }
}

/// On-disk shape: `[[entry]]` tables with optional `case_sensitive` /
/// `whole_word` keys.
#[derive(Debug, Serialize, Deserialize)]
struct PronunciationsFile {
    #[serde(default)]
    entry: Vec<PronunciationEntry>,
}

/// True when `rule` matches the text at char offset `index`.
fn rule_matches(rule: &Rule, chars: &[char], index: usize) -> bool {
    if chars.len() - index < rule.len {
        return false;
    }
    let candidate: String = chars[index..index + rule.len].iter().collect();
    let equal = if rule.case_sensitive {
        candidate == rule.written
    } else {
        candidate.to_lowercase() == rule.lowered
    };
    if !equal {
        return false;
    }
    if !rule.whole_word {
        return true;
    }
    if index > 0 && chars[index - 1].is_alphanumeric() {
        return false;
    }
    if index + rule.len < chars.len() && chars[index + rule.len].is_alphanumeric() {
        return false;
    }
    true
}

/// `pronunciations.toml` next to the settings file.
pub fn pronunciations_path() -> Option<PathBuf> {
    settings_path().and_then(|path| path.parent().map(|dir| dir.join("pronunciations.toml")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_temp_dir() -> PathBuf {
        let id = TEST_DIR_COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!(
            "smart-voices-pronounce-test-{}-{id}",
            std::process::id()
        ))
    }

    fn dict(entries: Vec<PronunciationEntry>) -> Pronunciations {
        Pronunciations::new(entries)
    }

    #[test]
    fn whole_word_skips_substrings() {
        let dictionary = dict(vec![PronunciationEntry::new("SQL", "sequel")]);
        assert_eq!(dictionary.apply("I love SQL."), "I love sequel.");
        assert_eq!(
            dictionary.apply("MySQL is a database."),
            "MySQL is a database."
        );
        assert_eq!(
            dictionary.apply("SQL, TTS and APIs."),
            "sequel, TTS and APIs."
        );
    }

    #[test]
    fn substring_entry_replaces_inside_words() {
        let dictionary = dict(vec![PronunciationEntry {
            whole_word: false,
            ..PronunciationEntry::new("sql", "sequel")
        }]);
        assert_eq!(dictionary.apply("MySQL"), "Mysequel");
    }

    #[test]
    fn case_insensitive_by_default() {
        let dictionary = dict(vec![PronunciationEntry::new("API", "A P I")]);
        assert_eq!(
            dictionary.apply("the api returns json"),
            "the A P I returns json"
        );
    }

    #[test]
    fn case_sensitive_entry_keeps_case() {
        let dictionary = dict(vec![PronunciationEntry {
            case_sensitive: true,
            ..PronunciationEntry::new("API", "A P I")
        }]);
        assert_eq!(
            dictionary.apply("the api returns data"),
            "the api returns data"
        );
        assert_eq!(
            dictionary.apply("the API returns data"),
            "the A P I returns data"
        );
    }

    #[test]
    fn longest_match_wins() {
        let dictionary = dict(vec![
            PronunciationEntry::new("GitHub", "git hub"),
            PronunciationEntry::new("GitHub Actions", "git hub actions pipeline"),
        ]);
        assert_eq!(
            dictionary.apply("GitHub Actions is great."),
            "git hub actions pipeline is great."
        );
        assert_eq!(
            dictionary.apply("I use GitHub daily."),
            "I use git hub daily."
        );
    }

    #[test]
    fn output_is_not_reprocessed() {
        let dictionary = dict(vec![
            PronunciationEntry {
                whole_word: false,
                ..PronunciationEntry::new("x", "yy")
            },
            PronunciationEntry {
                whole_word: false,
                ..PronunciationEntry::new("yy", "z")
            },
        ]);
        assert_eq!(dictionary.apply("x"), "yy");
    }

    #[test]
    fn empty_written_is_ignored() {
        let dictionary = dict(vec![PronunciationEntry::new("", "oops")]);
        assert_eq!(dictionary.apply("hello"), "hello");
    }

    #[test]
    fn default_seeds() {
        let defaults = PronunciationEntry::defaults();
        let spoken = |written: &str| {
            defaults
                .iter()
                .find(|entry| entry.written == written)
                .map(|entry| entry.spoken.as_str())
        };
        assert_eq!(defaults.len(), 10);
        assert_eq!(spoken("SQL"), Some("sequel"));
        assert_eq!(spoken("macOS"), Some("mac O S"));
        assert_eq!(spoken("iOS"), Some("eye O S"));
        assert_eq!(spoken("GitHub"), Some("git hub"));
        assert_eq!(spoken("LLM"), Some("L L M"));
        assert_eq!(spoken("TTS"), Some("T T S"));
        assert_eq!(spoken("API"), Some("A P I"));
        assert_eq!(spoken("UI"), Some("U I"));
        assert_eq!(spoken("JSON"), Some("jason"));
        assert_eq!(spoken("Kubernetes"), Some("koo ber net ees"));
    }

    #[test]
    fn toml_round_trip() {
        let dir = unique_temp_dir();
        let path = dir.join("pronunciations.toml");
        let dictionary = Pronunciations::with_defaults();
        dictionary.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[[entry]]"), "{text}");
        assert!(text.contains("written = \"SQL\""), "{text}");
        let (loaded, err) = Pronunciations::load(&path);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(err.is_none(), "unexpected error: {err:?}");
        assert_eq!(loaded.entries(), dictionary.entries());
    }

    #[test]
    fn optional_flags_default_on_load() {
        let dir = unique_temp_dir();
        let path = dir.join("pronunciations.toml");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "[[entry]]\nwritten = \"SQL\"\nspoken = \"sequel\"\n").unwrap();
        let (loaded, err) = Pronunciations::load(&path);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(err.is_none(), "unexpected error: {err:?}");
        assert_eq!(loaded.entries(), [PronunciationEntry::new("SQL", "sequel")]);
    }

    #[test]
    fn missing_file_writes_defaults() {
        let dir = unique_temp_dir();
        let path = dir.join("sub").join("pronunciations.toml");
        let (loaded, err) = Pronunciations::load(&path);
        let seeded = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(err.is_none(), "unexpected error: {err:?}");
        assert_eq!(loaded.entries(), Pronunciations::with_defaults().entries());
        assert!(seeded.contains("written = \"Kubernetes\""), "{seeded}");
    }

    #[test]
    fn invalid_file_uses_defaults_without_overwriting() {
        let dir = unique_temp_dir();
        let path = dir.join("pronunciations.toml");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "[[entry]\nwritten = [unclosed\n").unwrap();
        let before = std::fs::read_to_string(&path).unwrap();
        let (loaded, err) = Pronunciations::load(&path);
        let after = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(loaded.entries(), Pronunciations::with_defaults().entries());
        let err = err.expect("expected an error for invalid TOML");
        let msg = err.to_string();
        assert!(msg.contains(&path.to_string_lossy().into_owned()), "{msg}");
        assert!(
            msg.contains("Using the built-in list until it's fixed"),
            "{msg}"
        );
        assert_eq!(before, after, "broken file must not be overwritten");
    }
}
