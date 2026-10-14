//! Sentence-bounded chunking for TTS (port of `SpeechChunker.swift`).
//!
//! Port of `Sources/SmartVoicesCore/SpeechChunker.swift`: same chunk shapes
//! and split-before/after-limit logic (counts are in chars, not bytes), but
//! with a short-first schedule (50/150/300) for fast first audio instead of
//! Swift's 120/300. Sentence splitting differs by design: Swift uses
//! `NLTokenizer`, replaced here by a small hand-written splitter (no crate)
//! that breaks after sentence punctuation unless it ends a known abbreviation
//! or an initial.

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
    second_chunk_max_characters: usize,
    natural_first: bool,
}

impl Default for SpeechChunker {
    fn default() -> Self {
        Self {
            max_characters: 300,
            first_chunk_max_characters: 50,
            second_chunk_max_characters: 150,
            natural_first: true,
        }
    }
}

impl SpeechChunker {
    /// Chunker with the default schedule (50 chars for chunk 1, 150 for
    /// chunk 2, 300 after) for fast first audio.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Chunker with custom limits (chars, not bytes). The second chunk uses
    /// `max_characters` (the old "first vs rest" shape, kept for tests); the
    /// chunk-1 natural break is off so the old expectations hold.
    #[must_use]
    pub fn with_limits(max_characters: usize, first_chunk_max_characters: usize) -> Self {
        Self {
            max_characters,
            first_chunk_max_characters,
            second_chunk_max_characters: max_characters,
            natural_first: false,
        }
    }

    /// Split segments into chunks. The last chunk of each segment keeps the
    /// segment's pause; earlier chunks get [`SpeechPause::None`].
    #[must_use]
    pub fn chunks(&self, segments: &[SpeechSegment]) -> Vec<SpeechChunk> {
        let mut result = Vec::new();
        let mut chunk_index = 0;
        for segment in segments {
            let trimmed = segment.text.trim();
            if trimmed.is_empty() {
                continue;
            }
            let mut texts: Vec<String> = Vec::new();
            for sentence in split_sentences(trimmed) {
                texts.extend(self.split_sentence(&sentence, &mut chunk_index));
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
    fn split_sentence(&self, sentence: &str, chunk_index: &mut usize) -> Vec<String> {
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
            let limit = match *chunk_index {
                0 => self.first_chunk_max_characters,
                1 => self.second_chunk_max_characters,
                _ => self.max_characters,
            };
            if remaining.chars().count() <= limit {
                parts.push(remaining);
                *chunk_index += 1;
                break;
            }
            // Chunk 1 prefers a natural break in 12..=80 chars, even past 50.
            if *chunk_index == 0
                && self.natural_first
                && let Some((first, rest)) = split_first_chunk_natural(&remaining)
            {
                parts.push(first.trim().to_string());
                *chunk_index += 1;
                remaining = rest;
                continue;
            }
            if let Some((first, rest)) = split_before_limit(&remaining, limit) {
                parts.push(first.trim().to_string());
                *chunk_index += 1;
                remaining = rest;
            } else if let Some((first, rest)) = split_after_limit(&remaining, limit) {
                parts.push(first.trim().to_string());
                *chunk_index += 1;
                remaining = rest;
            } else {
                // Single long word: emit whole to avoid mid-word split.
                parts.push(remaining);
                *chunk_index += 1;
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

/// Split chunk 1 at a natural break (", ", "; ", ": " or " — ") whose first
/// part is 12..=80 chars. Prefers the longest break at or under 50 chars,
/// else the shortest break past 50. Punctuation stays in the first part.
fn split_first_chunk_natural(text: &str) -> Option<(String, String)> {
    const MIN_FIRST: usize = 12;
    const TARGET: usize = 50;
    const MAX_FIRST: usize = 80;
    let chars: Vec<char> = text.chars().collect();
    let mut best_under: Option<(usize, usize, usize)> = None;
    let mut best_over: Option<(usize, usize, usize)> = None;
    for separator in [", ", "; ", ": ", " — "] {
        let needle: Vec<char> = separator.chars().collect();
        let mut from = 0;
        while from + needle.len() <= chars.len() {
            let Some(found) = find_seq(&chars[from..], &needle) else {
                break;
            };
            let at = from + found;
            let first_len = at + needle.len() - 1;
            if (MIN_FIRST..=MAX_FIRST).contains(&first_len) {
                let first: String = chars[..first_len].iter().collect();
                if !first.trim().is_empty() {
                    if first_len <= TARGET {
                        let better = best_under.is_none_or(|(len, _, _)| first_len > len);
                        if better {
                            best_under = Some((first_len, at, needle.len()));
                        }
                    } else {
                        let better = best_over.is_none_or(|(len, _, _)| first_len < len);
                        if better {
                            best_over = Some((first_len, at, needle.len()));
                        }
                    }
                }
            }
            from = at + 1;
        }
    }
    let (first_len, at, needle_len) = best_under.or(best_over)?;
    let first: String = chars[..first_len].iter().collect();
    let rest: String = chars[at + needle_len..].iter().collect();
    if rest.trim().is_empty() {
        return None;
    }
    Some((first, rest))
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

    #[test]
    fn first_sentence_with_comma_at_46_splits_there() {
        let chunker = SpeechChunker::new();
        let text = format!("{}, {}.", "a".repeat(46), "b".repeat(65));
        assert_eq!(text.chars().count(), 114);
        let result = chunker.chunks(&[seg(&text, SpeechPause::Medium)]);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].text, format!("{},", "a".repeat(46)));
        assert_eq!(result[1].text, format!("{}.", "b".repeat(65)));
    }

    #[test]
    fn thirty_char_first_sentence_stays_whole() {
        let chunker = SpeechChunker::new();
        let text = "This sentence is thirty chars.";
        assert_eq!(text.chars().count(), 30);
        let result = chunker.chunks(&[seg(text, SpeechPause::Medium)]);
        assert_eq!(result, [chunk(text, SpeechPause::Medium)]);
    }

    #[test]
    fn second_chunk_respects_150() {
        let chunker = SpeechChunker::new();
        let long = format!("{}.", "w ".repeat(100).trim_end());
        assert!(long.chars().count() > 150);
        let text = format!("Short first. {long}");
        let result = chunker.chunks(&[seg(&text, SpeechPause::Medium)]);
        assert!(result.len() >= 3, "expected chunk 2 to split: {result:?}");
        assert_eq!(result[0].text, "Short first.");
        assert!(
            result[1].text.chars().count() <= 150,
            "chunk 2 is {} chars",
            result[1].text.chars().count()
        );
    }

    #[test]
    fn first_chunk_stays_short_for_long_paragraph_without_commas() {
        let chunker = SpeechChunker::new();
        let paragraph = format!("{}.", "word ".repeat(80).trim_end());
        assert_eq!(paragraph.chars().count(), 400);
        assert!(!paragraph.contains(','));
        let result = chunker.chunks(&[seg(&paragraph, SpeechPause::Medium)]);
        assert!(!result.is_empty());
        assert!(
            result[0].text.chars().count() <= 80,
            "chunk 1 is {} chars",
            result[0].text.chars().count()
        );
    }
}
