//! Pocket TTS v2 text front end: model-wrapper normalization, sentence
//! splitting, G2P cleaning, phoneme translation, and ezafe-aware chunking.
//!
//! Rust port of readly's `PocketTextV2.kt` (itself a port of
//! `normalize_for_model`, the HARD_BREAK split, `clean_sentence_for_g2p`,
//! `translate_phonemes` and the ezafe-aware `split_text`/`strip_ezafe` from
//! the v2 reference). The v2 `normalize()` is identical to v1's, so
//! normalization itself reuses [`normalize`](super::normalize_fa::normalize).
//! Checked against the reference on every golden row
//! (`tests/fixtures/pocket/v2-golden.jsonl`).

use std::fmt;

use super::normalize_fa::{is_persian_letter, normalize};

/// Highest token budget per v2 chunk (reference `_synth_unit`).
pub(crate) const MAX_TOKENS: usize = 18;

/// Lowest token budget per v2 chunk (reference `_synth_unit`).
pub(crate) const MIN_TOKENS: usize = 8;

/// Homo-GE2PE marks the ezafe with a trailing "1" (reference `synthesize`).
const EZAFE_MARK: char = '1';

const HARD_BREAKS: [char; 3] = ['.', '!', '؟'];
const SOFT_BREAKS: [char; 3] = ['،', '؛', ':'];
const BREAK_CHARS: [char; 6] = ['.', '!', '؟', '،', '؛', ':'];

/// readly's `UNICODE_WS`: Unicode whitespace plus FS/GS/RS/US.
fn is_v2_ws(c: char) -> bool {
    c.is_whitespace() || matches!(c, '\u{1c}'..='\u{1f}')
}

fn strip(text: &str) -> &str {
    text.trim_matches(is_v2_ws)
}

fn split_words(text: &str) -> Vec<&str> {
    text.split(is_v2_ws).filter(|word| !word.is_empty()).collect()
}

/// Split after a break char followed by whitespace (the `HARD_BREAK` /
/// `SOFT_BREAK` regexes: `(?<=breaks)WS+`), stripped, empties dropped.
fn split_keep_all(text: &str, breaks: &[char]) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < chars.len() {
        if breaks.contains(&chars[i]) && i + 1 < chars.len() && is_v2_ws(chars[i + 1]) {
            let raw: String = chars[start..=i].iter().collect();
            let piece = strip(&raw);
            if !piece.is_empty() {
                parts.push(piece.to_string());
            }
            i += 1;
            while i < chars.len() && is_v2_ws(chars[i]) {
                i += 1;
            }
            start = i;
        } else {
            i += 1;
        }
    }
    if start < chars.len() {
        let raw: String = chars[start..].iter().collect();
        let piece = strip(&raw);
        if !piece.is_empty() {
            parts.push(piece.to_string());
        }
    }
    parts
}

/// True when `text` is already phonemes rather than Persian script: no
/// Persian letter, but some ASCII letter (reference `is_phonemic`).
fn is_phonemic(text: &str) -> bool {
    text.chars().all(|c| !is_persian_letter(c)) && text.chars().any(|c| c.is_ascii_alphabetic())
}

/// Persian input that normalizes to empty (and wasn't blank): outside the
/// alphabet, so speaking it would hide a broken pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmptyAfterNormalize(String);

impl fmt::Display for EmptyAfterNormalize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let preview: String = self.0.chars().take(60).collect();
        write!(
            f,
            "normalisation emptied the text: \"{preview}\". If this is phoneme input, it was not recognised as such; if it is Persian, it is outside the alphabet."
        )
    }
}

impl std::error::Error for EmptyAfterNormalize {}

/// Normalise Persian input, or pass phonemes through untouched (reference
/// `normalize_for_model`). Errors when Persian input normalizes to empty,
/// so a silent empty generation never hides a broken pipeline.
pub fn normalize_for_model(text: &str) -> Result<String, EmptyAfterNormalize> {
    if is_phonemic(text) {
        return Ok(text.to_string());
    }
    let out = normalize(text);
    if !text.chars().all(is_v2_ws) && out.is_empty() {
        return Err(EmptyAfterNormalize(text.to_string()));
    }
    Ok(out)
}

/// Split normalized text into sentences on HARD_BREAK, stripped, empties
/// dropped (reference `synth_one` v2 branch).
pub fn split_sentences(normalized: &str) -> Vec<String> {
    split_keep_all(normalized, &HARD_BREAKS)
}

/// Strip question marks the G2P model was not trained on.
pub fn clean_sentence_for_g2p(sentence: &str) -> String {
    sentence.replace(['؟', '?'], "")
}

/// Map Homo-GE2PE output symbols onto the pocket-tts v2 phone set.
pub fn translate_phonemes(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '/' => 'a',
            'a' => 'A',
            '@' => '?',
            '$' => 'S',
            'c' => 'C',
            other => other,
        })
        .collect()
}

/// The text as the model must see it: no ezafe marks.
pub(crate) fn strip_ezafe(text: &str) -> String {
    text.replace(EZAFE_MARK, "")
}

/// Split phoneme `text` (ezafe marks kept) into chunks of at most
/// `max_tokens` tokens: sentence boundaries first, then clause punctuation,
/// then evenly packed words — a port of `split_text` in `synthesize.py`
/// with `keep_punct_boundaries=false` (the `_synth_unit` call).
/// `count_tokens` sees the same text the model will (callers strip the
/// ezafe marks); a chunk boundary never lands between a word and the
/// complement its ezafe binds it to, and no chunk stays under `min_tokens`
/// while words can move back from its neighbour.
pub(crate) fn split_chunks(
    text: &str,
    count_tokens: &dyn Fn(&str) -> usize,
    max_tokens: usize,
    min_tokens: usize,
) -> Vec<String> {
    let clean = split_words(text).join(" ");
    if clean.is_empty() {
        return Vec::new();
    }
    let parts = split_recurse(&clean, count_tokens, max_tokens);
    let mut merged: Vec<String> = Vec::new();
    for chunk in parts {
        if let Some(last) = merged.last() {
            let trial = format!("{last} {chunk}");
            if count_tokens(&trial) <= max_tokens {
                *merged.last_mut().expect("just checked") = trial;
                continue;
            }
        }
        merged.push(chunk);
    }
    for i in (1..merged.len()).rev() {
        if count_tokens(&merged[i]) >= min_tokens {
            continue;
        }
        if ends_with_break(&merged[i - 1]) {
            continue;
        }
        while count_tokens(&merged[i]) < min_tokens {
            let prev_words: Vec<String> =
                split_words(&merged[i - 1]).iter().map(|s| s.to_string()).collect();
            if prev_words.len() < 2 {
                break;
            }
            let rest = prev_words[..prev_words.len() - 1].join(" ");
            if count_tokens(&rest) < min_tokens {
                break;
            }
            let moved = prev_words[prev_words.len() - 1].clone();
            merged[i - 1] = rest;
            let grown = format!("{moved} {}", merged[i]);
            merged[i] = grown;
        }
    }
    merged
}

fn split_recurse(
    piece: &str,
    count_tokens: &dyn Fn(&str) -> usize,
    max_tokens: usize,
) -> Vec<String> {
    if count_tokens(piece) <= max_tokens {
        return vec![piece.to_string()];
    }
    for breaks in [HARD_BREAKS.as_slice(), SOFT_BREAKS.as_slice()] {
        let parts = split_keep_all(piece, breaks);
        if parts.len() > 1 {
            return parts
                .iter()
                .flat_map(|part| split_recurse(part, count_tokens, max_tokens))
                .collect();
        }
    }
    let words = split_words(piece);
    let total = count_tokens(piece);
    let mut chunks = total.div_ceil(max_tokens).max(1);
    let mut packed: Vec<String> = Vec::new();
    for _ in 0..4 {
        packed = pack(&words, count_tokens, max_tokens, chunks, total.div_ceil(chunks));
        if packed.len() <= chunks {
            return packed;
        }
        chunks = packed.len();
    }
    packed
}

fn pack(
    words: &[&str],
    count_tokens: &dyn Fn(&str) -> usize,
    max_tokens: usize,
    n_chunks: usize,
    target: usize,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    for word in words {
        let trial = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };
        let owed = out.len() < n_chunks.saturating_sub(1);
        let bound = !current.is_empty()
            && split_words(&current)
                .last()
                .is_some_and(|last| last.ends_with(EZAFE_MARK));
        if !current.is_empty()
            && !bound
            && (count_tokens(&trial) > max_tokens || (owed && count_tokens(&trial) > target))
        {
            out.push(std::mem::take(&mut current));
            current = word.to_string();
        } else {
            current = trial;
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn ends_with_break(chunk: &str) -> bool {
    let stripped = chunk.trim_end_matches(is_v2_ws);
    stripped.chars().last().is_some_and(|c| BREAK_CHARS.contains(&c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_g2p_symbols_to_phones() {
        assert_eq!(translate_phonemes("ketabe1 j/dide1 m/n @/st"), "ketAbe1 jadide1 man ?ast");
        assert_eq!(translate_phonemes("hami$e cera"), "hAmiSe CerA");
        assert_eq!(translate_phonemes(""), "");
    }

    #[test]
    fn cleaner_strips_question_marks_and_ezafe_strips_marks() {
        assert_eq!(clean_sentence_for_g2p("کی آمد؟"), "کی آمد");
        assert_eq!(clean_sentence_for_g2p("who?"), "who");
        assert_eq!(strip_ezafe("ketAbe1 jadide1 miz"), "ketAbe jadide miz");
    }

    #[test]
    fn normalize_for_model_passes_phonemes_and_rejects_empty() {
        assert_eq!(normalize_for_model("ketAb jadid").unwrap(), "ketAb jadid");
        assert_eq!(normalize_for_model("hello").unwrap(), "hello");
        assert_eq!(normalize_for_model("  ").unwrap(), "");
        let err = normalize_for_model("...").expect_err("punct-only must fail");
        assert_eq!(
            err.to_string(),
            "normalisation emptied the text: \"...\". If this is phoneme input, it was not recognised as such; if it is Persian, it is outside the alphabet."
        );
        assert!(!is_phonemic("سلام ketAb"));
        assert!(is_phonemic("salam 123"));
    }

    #[test]
    fn splits_sentences_on_hard_breaks() {
        assert_eq!(
            split_sentences("چی شده؟ چرا انقدر ناراحتی؟"),
            vec!["چی شده؟", "چرا انقدر ناراحتی؟"]
        );
        assert_eq!(split_sentences("کی آمد؟"), vec!["کی آمد؟"]);
        assert!(split_sentences("").is_empty());
        assert!(split_sentences("   ").is_empty());
    }

    #[test]
    fn chunk_boundaries_never_split_an_ezafe_chain() {
        // Word counter; every word but the last carries an ezafe mark, so no
        // boundary may land inside the chain whatever the budget says.
        let counter = |s: &str| s.split_whitespace().count();
        let chunks = split_chunks("ketAbe1 jadide1 duste1 man", &counter, 3, 2);
        assert_eq!(chunks, vec!["ketAbe1 jadide1 duste1 man"]);
    }

    #[test]
    fn short_tail_moves_words_back_unless_after_a_break() {
        let counter = |s: &str| s.split_whitespace().count();
        // The ezafe chain packs unevenly ("a1 b1 c" | "d", unmergeable), so
        // the short tail pulls "c" back.
        assert_eq!(
            split_chunks("a1 b1 c d", &counter, 3, 2),
            vec!["a1 b1", "c d"]
        );
        // After a real break the short tail stays put.
        assert_eq!(
            split_chunks("w1 w2 w3. w4", &counter, 3, 2),
            vec!["w1 w2 w3.", "w4"]
        );
    }
}
