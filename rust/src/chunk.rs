//! Sentence-bounded chunking for TTS (port of `SpeechChunker.swift`).
//!
//! Faithful port of `Sources/SmartVoicesCore/SpeechChunker.swift`: same chunk
//! shapes, limits and split-before/after-limit logic (counts are in chars, not
//! bytes). Sentence splitting differs by design: Swift uses `NLTokenizer`,
//! replaced here by a small hand-written splitter (no crate) that breaks after
//! sentence punctuation unless it ends a known abbreviation or an initial.

use crate::sanitize::{SpeechPause, SpeechSegment};

/// A chunk of speakable text with its trailing pause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeechChunk {
    pub text: String,
    pub pause_after: SpeechPause,
}

/// Splits segments into sentence-bounded chunks for TTS.
#[derive(Debug, Clone)]
pub struct SpeechChunker {
    max_characters: usize,
    first_chunk_max_characters: usize,
}

impl Default for SpeechChunker {
    fn default() -> Self {
        Self {
            max_characters: 300,
            first_chunk_max_characters: 120,
        }
    }
}

impl SpeechChunker {
    /// Chunker with the default limits (300 chars, 120 for the first chunk).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Chunker with custom limits (chars, not bytes).
    #[must_use]
    pub fn with_limits(max_characters: usize, first_chunk_max_characters: usize) -> Self {
        Self {
            max_characters,
            first_chunk_max_characters,
        }
    }

    /// Split segments into chunks. The last chunk of each segment keeps the
    /// segment's pause; earlier chunks get [`SpeechPause::None`].
    #[must_use]
    pub fn chunks(&self, segments: &[SpeechSegment]) -> Vec<SpeechChunk> {
        let mut result = Vec::new();
        let mut is_first_chunk = true;
        for segment in segments {
            let trimmed = segment.text.trim();
            if trimmed.is_empty() {
                continue;
            }
            let mut texts: Vec<String> = Vec::new();
            for sentence in split_sentences(trimmed) {
                texts.extend(self.split_sentence(&sentence, &mut is_first_chunk));
            }
            let last = texts.len().saturating_sub(1);
            for (index, text) in texts.into_iter().enumerate() {
                let pause = if index == last {
                    segment.pause
                } else {
                    SpeechPause::None
                };
                result.push(SpeechChunk {
                    text,
                    pause_after: pause,
                });
            }
        }
        result
    }

    /// Split one sentence into limit-sized parts.
    fn split_sentence(&self, sentence: &str, is_first: &mut bool) -> Vec<String> {
        let mut remaining = sentence.trim().to_string();
        if remaining.is_empty() {
            return Vec::new();
        }
        let mut parts = Vec::new();
        while !remaining.is_empty() {
            remaining = remaining.trim().to_string();
            if remaining.is_empty() {
                break;
            }
            let limit = if *is_first {
                self.first_chunk_max_characters
            } else {
                self.max_characters
            };
            if remaining.chars().count() <= limit {
                parts.push(remaining);
                *is_first = false;
                break;
            }
            if let Some((first, rest)) = split_before_limit(&remaining, limit) {
                parts.push(first.trim().to_string());
                *is_first = false;
                remaining = rest;
            } else if let Some((first, rest)) = split_after_limit(&remaining, limit) {
                parts.push(first.trim().to_string());
                *is_first = false;
                remaining = rest;
            } else {
                // Single long word: emit whole to avoid mid-word split.
                parts.push(remaining);
                *is_first = false;
                break;
            }
        }
        parts
    }
}

/// Split at the last `"; "`, `", "` or `" "` within the first `limit` chars.
/// Keeps `;`/`,` in the first part; the space is dropped either way.
fn split_before_limit(text: &str, limit: usize) -> Option<(String, String)> {
    let chars: Vec<char> = text.chars().collect();
    if limit == 0 || chars.len() <= limit {
        return None;
    }
    let prefix = &chars[..limit];
    for separator in ["; ", ", ", " "] {
        let needle: Vec<char> = separator.chars().collect();
        if let Some(at) = rfind_seq(prefix, &needle) {
            let (first, rest) = split_at(&chars, at, &needle);
            if !first.trim().is_empty() {
                return Some((first, rest));
            }
        }
    }
    None
}

/// Split at the first `"; "`, `", "` or `" "` at or after `limit` chars.
fn split_after_limit(text: &str, limit: usize) -> Option<(String, String)> {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= limit {
        return None;
    }
    let suffix = &chars[limit..];
    for separator in ["; ", ", ", " "] {
        let needle: Vec<char> = separator.chars().collect();
        if let Some(found) = find_seq(suffix, &needle) {
            return Some(split_at(&chars, limit + found, &needle));
        }
    }
    None
}

/// Split `chars` around `needle` at `at`: a two-char separator keeps its
/// punctuation in the first part; the space is dropped either way.
fn split_at(chars: &[char], at: usize, needle: &[char]) -> (String, String) {
    let first: String = if needle.len() == 2 {
        chars[..at + 1].iter().collect()
    } else {
        chars[..at].iter().collect()
    };
    let rest: String = chars[at + needle.len()..].iter().collect();
    (first, rest)
}

/// Last index of `needle` in `haystack` (char slices, never byte offsets).
fn rfind_seq(haystack: &[char], needle: &[char]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (0..=haystack.len() - needle.len())
        .rev()
        .find(|&i| haystack[i..i + needle.len()] == *needle)
}

/// First index of `needle` in `haystack` (char slices, never byte offsets).
fn find_seq(haystack: &[char], needle: &[char]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (0..=haystack.len() - needle.len()).find(|&i| haystack[i..i + needle.len()] == *needle)
}

// MARK: - Sentence splitter

/// Split `text` into sentences: break after `.` `!` `?` `…`, optionally
/// followed by closing quotes/brackets, when followed by whitespace or the end
/// of input. The CJK/Arabic marks `。！？؟` need no following space. No break
/// after a known abbreviation or a single uppercase-letter initial.
fn split_sentences(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut sentences = Vec::new();
    let mut start = 0;
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        if is_sentence_terminal(c) {
            let mut end = index + 1;
            while end < chars.len() && is_closing(chars[end]) {
                end += 1;
            }
            let at_end = end >= chars.len();
            let followed_by_space = !at_end && chars[end].is_whitespace();
            let cjk = matches!(c, '。' | '！' | '？' | '؟');
            if (cjk || at_end || followed_by_space)
                && !(c == '.' && is_abbreviation_before(&chars, index))
            {
                let sentence: String = chars[start..end].iter().collect();
                if !sentence.trim().is_empty() {
                    sentences.push(sentence.trim().to_string());
                }
                start = end;
                while start < chars.len() && chars[start].is_whitespace() {
                    start += 1;
                }
                index = start;
                continue;
            }
        }
        index += 1;
    }
    let tail: String = chars[start..].iter().collect();
    if !tail.trim().is_empty() {
        sentences.push(tail.trim().to_string());
    }
    sentences
}

/// Sentence-ending punctuation.
fn is_sentence_terminal(c: char) -> bool {
    matches!(c, '.' | '!' | '?' | '…' | '。' | '！' | '？' | '؟')
}

/// Closing quotes/brackets that may follow sentence punctuation.
fn is_closing(c: char) -> bool {
    matches!(c, '"' | '\'' | '”' | '’' | ')' | ']')
}

/// Abbreviations (lowercase) that never end a sentence.
const ABBREVIATIONS: &[&str] = &[
    "mr", "mrs", "ms", "dr", "prof", "sr", "jr", "st", "vs", "etc", "e.g", "i.e", "inc", "ltd",
    "co", "fig", "no", "vol", "approx",
];

/// True when the `.` at `dot` ends an abbreviation (case-insensitive, matched
/// as the whole word before it) or a single uppercase-letter initial.
fn is_abbreviation_before(chars: &[char], dot: usize) -> bool {
    let mut start = dot;
    while start > 0 && (chars[start - 1].is_alphanumeric() || chars[start - 1] == '.') {
        start -= 1;
    }
    let word: String = chars[start..dot].iter().collect();
    let word = word.trim_matches('.');
    if word.is_empty() {
        return false;
    }
    if ABBREVIATIONS.contains(&word.to_lowercase().as_str()) {
        return true;
    }
    let mut word_chars = word.chars();
    matches!(
        (word_chars.next(), word_chars.next()),
        (Some(c), None) if c.is_uppercase()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(text: &str, pause: SpeechPause) -> SpeechSegment {
        SpeechSegment {
            text: text.to_string(),
            pause,
        }
    }

    fn chunk(text: &str, pause_after: SpeechPause) -> SpeechChunk {
        SpeechChunk {
            text: text.to_string(),
            pause_after,
        }
    }

    #[test]
    fn abbreviations_do_not_split() {
        let chunker = SpeechChunker::new();
        let result = chunker.chunks(&[seg("Dr. Smith went home.", SpeechPause::Medium)]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].text, "Dr. Smith went home.");
        assert_eq!(result[0].pause_after, SpeechPause::Medium);
    }

    #[test]
    fn decimals_do_not_split() {
        let chunker = SpeechChunker::new();
        let result = chunker.chunks(&[seg("The value is 3.5 meters.", SpeechPause::Short)]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].text, "The value is 3.5 meters.");
    }

    #[test]
    fn long_sentence_splits_at_semicolon() {
        let chunker = SpeechChunker::with_limits(30, 30);
        let text = "Apples are red; oranges are orange; bananas are yellow.";
        let result = chunker.chunks(&[seg(text, SpeechPause::Medium)]);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].text, "Apples are red;");
        assert_eq!(result[1].text, "oranges are orange;");
        assert_eq!(result[2].text, "bananas are yellow.");
        assert_eq!(
            result
                .iter()
                .map(|c| c.text.as_str())
                .collect::<Vec<_>>()
                .join(" "),
            text
        );
    }

    #[test]
    fn long_sentence_splits_at_comma() {
        let chunker = SpeechChunker::with_limits(20, 20);
        let text = "One, two, three, four, five, six, seven.";
        let result = chunker.chunks(&[seg(text, SpeechPause::Medium)]);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].text, "One, two, three,");
        assert_eq!(result[1].text, "four, five, six,");
        assert_eq!(result[2].text, "seven.");
        assert_eq!(
            result
                .iter()
                .map(|c| c.text.as_str())
                .collect::<Vec<_>>()
                .join(" "),
            text
        );
    }

    #[test]
    fn long_sentence_splits_at_space() {
        let chunker = SpeechChunker::with_limits(20, 20);
        let text = "Alpha beta gamma delta epsilon zeta eta theta.";
        let result = chunker.chunks(&[seg(text, SpeechPause::Medium)]);
        assert!(result.len() >= 2);
        for chunk in &result {
            assert!(chunk.text.chars().count() <= 20);
            assert!(!chunk.text.is_empty());
        }
        assert_eq!(
            result
                .iter()
                .map(|c| c.text.as_str())
                .collect::<Vec<_>>()
                .join(" "),
            text
        );
    }

    #[test]
    fn first_chunk_uses_first_limit() {
        let chunker = SpeechChunker::with_limits(300, 10);
        let result = chunker.chunks(&[seg("Hello world, this is a test.", SpeechPause::Medium)]);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].text, "Hello");
        assert!(result[0].text.chars().count() <= 10);
        assert_eq!(result[0].pause_after, SpeechPause::None);
        assert_eq!(result[1].pause_after, SpeechPause::Medium);
    }

    #[test]
    fn pause_carry_over() {
        let chunker = SpeechChunker::new();
        let result = chunker.chunks(&[
            seg("First.", SpeechPause::Short),
            seg("Second.", SpeechPause::Long),
        ]);
        assert_eq!(
            result,
            [
                chunk("First.", SpeechPause::Short),
                chunk("Second.", SpeechPause::Long),
            ]
        );
        let multi = chunker.chunks(&[seg("One. Two. Three.", SpeechPause::Medium)]);
        assert_eq!(multi.len(), 3);
        assert_eq!(multi[0].pause_after, SpeechPause::None);
        assert_eq!(multi[1].pause_after, SpeechPause::None);
        assert_eq!(multi[2].pause_after, SpeechPause::Medium);
    }

    #[test]
    fn empty_input_returns_empty() {
        let chunker = SpeechChunker::new();
        assert_eq!(chunker.chunks(&[]), []);
    }

    #[test]
    fn trims_and_skips_empty() {
        let chunker = SpeechChunker::new();
        let result = chunker.chunks(&[seg("  Hello.  ", SpeechPause::Medium)]);
        assert_eq!(result, [chunk("Hello.", SpeechPause::Medium)]);
    }

    #[test]
    fn initial_does_not_split() {
        let chunker = SpeechChunker::new();
        let result = chunker.chunks(&[seg("I met J. Smith. He waved.", SpeechPause::Medium)]);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].text, "I met J. Smith.");
        assert_eq!(result[1].text, "He waved.");
    }

    #[test]
    fn ellipsis_and_marks_split() {
        let chunker = SpeechChunker::new();
        let result = chunker.chunks(&[seg("Wait... what? Yes!", SpeechPause::Medium)]);
        assert_eq!(
            result.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(),
            ["Wait...", "what?", "Yes!"]
        );
    }

    #[test]
    fn persian_sentences_split() {
        let chunker = SpeechChunker::new();
        let result = chunker.chunks(&[seg("سلام. خوبی؟ بله.", SpeechPause::Medium)]);
        assert_eq!(
            result.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(),
            ["سلام.", "خوبی؟", "بله."]
        );
    }

    #[test]
    fn cjk_sentences_split() {
        let chunker = SpeechChunker::new();
        let result = chunker.chunks(&[seg("你好。再见。", SpeechPause::Medium)]);
        assert_eq!(
            result.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(),
            ["你好。", "再见。"]
        );
    }
}
