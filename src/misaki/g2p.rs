use super::{
    language::Language,
    lexicon::{Lexicon, TokenContext},
    tagger::PerceptronTagger,
};
use std::path::{Path, PathBuf};
pub struct G2P {
    lexicon: Lexicon,
    tagger: PerceptronTagger,
    data: PathBuf,
    british: bool,
}
impl G2P {
    pub fn load(root: &Path, data: &Path, british: bool) -> Result<Self, String> {
        Ok(Self {
            lexicon: Lexicon::new(
                if british {
                    Language::EnglishGB
                } else {
                    Language::EnglishUS
                },
                root,
            )?,
            tagger: PerceptronTagger::load(&root.join("tagger"))?,
            data: data.into(),
            british,
        })
    }
    pub fn g2p(&self, text: &str) -> Result<String, String> {
        self.g2p_with_notice(text).map(|p| p.0)
    }
    pub fn g2p_with_notice(&self, text: &str) -> Result<(String, bool), String> {
        let mut used_espeak = false;
        let result = self.g2p_using(text, |word| {
            used_espeak = true;
            crate::espeak::english(&self.data, word, self.british)
        })?;
        Ok((result, used_espeak))
    }
    fn g2p_using(
        &self,
        text: &str,
        mut pronounce: impl FnMut(&str) -> Result<String, String>,
    ) -> Result<String, String> {
        let re = regex::Regex::new(r"\p{L}+(?:['’]\p{L}+)*|[^\s]").map_err(|e| e.to_string())?;
        let mut tokens: Vec<(&str, usize, usize)> = Vec::new();
        let mut offset = 0;
        for word in text.split_whitespace() {
            let start = offset + text[offset..].find(word).ok_or("invalid word offset")?;
            offset = start + word.len();
            let core = word.trim_end_matches(['.', ',', '!', '?', ':', ';']);
            let initialism = word.contains('.')
                && word
                    .split('.')
                    .all(|p| p.len() <= 1 && p.chars().all(|c| c.is_ascii_alphabetic()));
            if self.lexicon.in_gold(word) || initialism {
                tokens.push((word, start, offset));
            } else if word.chars().any(|c| c.is_ascii_digit()) {
                tokens.push((core, start, start + core.len()));
                for m in re.find_iter(&word[core.len()..]) {
                    tokens.push((
                        m.as_str(),
                        start + core.len() + m.start(),
                        start + core.len() + m.end(),
                    ));
                }
            } else {
                for m in re.find_iter(word) {
                    tokens.push((m.as_str(), start + m.start(), start + m.end()));
                }
            }
        }
        let words: Vec<&str> = tokens.iter().map(|t| t.0).collect();
        let tags = self.tagger.tag(&words);
        let mut phonemes = vec![String::new(); tokens.len()];
        let mut ctx = TokenContext::default();
        for i in (0..tokens.len()).rev() {
            let word = tokens[i].0;
            // Punctuation is model input, not a word for espeak to pronounce.
            // Use codepoints: dashes, ellipsis and curly quotes are UTF-8.
            let ps = if word.chars().any(|c| c.is_ascii_digit()) {
                pronounce(word)?
            } else if word.chars().count() == 1 && ";:,.!?—…\"()“”".contains(word) {
                word.to_owned()
            } else if matches!(word, "-" | "–") {
                "—".to_owned()
            } else if matches!(word, "[" | "{") {
                "(".to_owned()
            } else if matches!(word, "]" | "}") {
                ")".to_owned()
            } else if let Some((p, _)) = self.lexicon.get_word(
                word,
                &tags[i],
                if word == word.to_lowercase() {
                    None
                } else if word == word.to_uppercase() {
                    Some(2.)
                } else {
                    Some(0.5)
                },
                Some(&ctx),
            ) {
                p
            } else {
                pronounce(word)?
            };
            ctx.future_vowel = ps.chars().find_map(|c| {
                if "AIOQWYaiuæɑɒɔəɛɜɪʊʌᵻ".contains(c) {
                    Some(true)
                } else if "bdfhjklmnpstvwzðŋɡɹɾʃʒʤʧθ".contains(c) {
                    Some(false)
                } else {
                    None
                }
            });
            ctx.future_to = word == "to";
            phonemes[i] = ps;
        }
        let mut out = String::new();
        for (i, p) in phonemes.iter().enumerate() {
            out.push_str(p);
            if i + 1 < tokens.len() && tokens[i].2 < tokens[i + 1].1 {
                out.push_str(&text[tokens[i].2..tokens[i + 1].1]);
            }
        }
        Ok(out.replace('ɾ', "T"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tiny() -> G2P {
        let lexicon = Lexicon {
            lang: Language::EnglishUS,
            cap_stresses: (0.5, 2.),
            golds: serde_json::from_str(
                r#"{"hello":"hello","world":"world","Dr.":"doctor","U":"jˈu","S":"ˈɛs","slash":"slæʃ","underscore":"ʌndəɹskɔɹ","to":"tu"}"#,
            )
            .unwrap(),
            silvers: serde_json::from_str("{}").unwrap(),
        };
        G2P {
            lexicon,
            tagger: PerceptronTagger::tiny(),
            data: PathBuf::new(),
            british: false,
        }
    }
    #[test]
    fn punctuation_and_abbreviations_keep_source_whitespace() {
        let g = tiny();
        assert_eq!(g.g2p("hello, world!").unwrap(), "hello, world!");
        assert_eq!(g.g2p("Dr. hello").unwrap(), "doctor hello");
        assert_eq!(g.g2p("U.S.").unwrap(), "jˌuˈɛs");
    }
    #[test]
    fn numeric_words_are_passed_whole_to_espeak() {
        let mut seen = Vec::new();
        let result = tiny()
            .g2p_using("$12.50 3:45 28th 75% 2.0 404.", |w| {
                seen.push(w.to_owned());
                Ok("n".into())
            })
            .unwrap();
        assert_eq!(seen, vec!["404", "2.0", "75%", "28th", "3:45", "$12.50"]);
        assert_eq!(result, "n n n n n n.");
    }
    #[test]
    fn model_punctuation_does_not_require_word_pronunciation() {
        // espeak returns no phonemes for punctuation. Passing any of these
        // symbols to it used to abort the rest of a reading.
        for punctuation in ";:,.!?—…\"()“”".chars() {
            let text = format!("hello {punctuation} world");
            let result = tiny()
                .g2p_using(&text, |_| Err("punctuation has no spoken phonemes".into()))
                .unwrap();
            assert_eq!(result, text);
        }
        for dash in ['-', '–'] {
            assert_eq!(
                tiny()
                    .g2p_using(&format!("hello{dash}world"), |_| Err(
                        "punctuation has no spoken phonemes".into()
                    ))
                    .unwrap(),
                "hello—world"
            );
        }
    }

    #[test]
    fn word_pronunciation_failure_still_stops_conversion() {
        assert_eq!(
            tiny().g2p_using("unknown", |_| Err("pronunciation unavailable".into())),
            Err("pronunciation unavailable".into())
        );
    }
    #[test]
    fn technical_text_punctuation_and_symbols_are_converted() {
        let result = tiny()
            .g2p_using("hello [world] {hello} / _ → world", |_| {
                Err("symbol has no spoken phonemes".into())
            })
            .unwrap();
        assert_eq!(result, "hello (world) (hello) slæʃ ʌndəɹskɔɹ tu world");
    }
    #[test]
    #[ignore = "requires local pronunciation data; run with MISAKI_TEST_ROOT and ESPEAK_TEST_ROOT"]
    fn python_regressions_and_converter_measurement() {
        let root = std::env::var("MISAKI_TEST_ROOT").expect("MISAKI_TEST_ROOT");
        let data = std::env::var("ESPEAK_TEST_ROOT").expect("ESPEAK_TEST_ROOT");
        let before = std::fs::read_to_string("/proc/self/status").unwrap();
        let t = std::time::Instant::now();
        let g = G2P::load(Path::new(&root), Path::new(&data), false).unwrap();
        println!(
            "converter load {:?}; before {} ; after {}",
            t.elapsed(),
            before.lines().find(|s| s.starts_with("VmHWM")).unwrap(),
            std::fs::read_to_string("/proc/self/status")
                .unwrap()
                .lines()
                .find(|s| s.starts_with("VmHWM"))
                .unwrap()
        );
        let fixtures: Vec<(String, String)> =
            serde_json::from_str(include_str!("python-fixtures.json")).unwrap();
        let mut failures = Vec::new();
        for (text, expected) in fixtures {
            let actual = g.g2p(&text).unwrap();
            if actual != expected {
                failures.push((text, expected, actual));
            }
        }
        assert!(failures.is_empty(), "Python deviations: {failures:#?}");
    }
}
