//! Estimated word timing for the reading panel's word highlight.
//!
//! The voice gives no word timestamps, so the current word is estimated from
//! the elapsed playback time: each word weighs `chars + 1 + 0.5 × punctuation
//! chars` (port of `WordTiming` from the old Swift app).

use std::ops::Range;

/// Non-ASCII punctuation that slows a word down (beyond
/// [`char::is_ascii_punctuation`]).
const EXTRA_PUNCTUATION: [char; 12] = ['،', '؛', '؟', '…', '—', '–', '«', '»', '“', '”', '‘', '’'];

fn is_punctuation(c: char) -> bool {
    c.is_ascii_punctuation() || EXTRA_PUNCTUATION.contains(&c)
}

/// Byte ranges of the words in `text`, split on [`char::is_whitespace`].
#[must_use]
pub fn word_ranges(text: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut start: Option<usize> = None;
    for (index, c) in text.char_indices() {
        if c.is_whitespace() {
            if let Some(word_start) = start.take() {
                ranges.push(word_start..index);
            }
        } else if start.is_none() {
            start = Some(index);
        }
    }
    if let Some(word_start) = start {
        ranges.push(word_start..text.len());
    }
    ranges
}

/// Weight of one word: `chars + 1 + 0.5 × punctuation chars`.
fn weight(word: &str) -> f64 {
    let chars = word.chars().count();
    let punctuation = word.chars().filter(|c| is_punctuation(*c)).count();
    (chars + 1) as f64 + 0.5 * (punctuation as f64)
}

/// Estimated index of the word spoken at `elapsed` seconds into a chunk of
/// `duration` seconds: the first word whose running weight total exceeds
/// `elapsed / duration × total`. Returns `None` when there are no words, when
/// `elapsed`/`duration` are negative, zero, NaN, or when `elapsed > duration`.
/// When `elapsed == duration` it returns the last word.
#[must_use]
pub fn word_index(text: &str, elapsed: f64, duration: f64) -> Option<usize> {
    let words: Vec<&str> = text
        .split(char::is_whitespace)
        .filter(|word| !word.is_empty())
        .collect();
    if words.is_empty() {
        return None;
    }
    if elapsed.is_nan() || elapsed < 0.0 || duration.is_nan() || duration <= 0.0 {
        return None;
    }
    if elapsed > duration {
        return None;
    }
    if elapsed == duration {
        return Some(words.len() - 1);
    }
    let total: f64 = words.iter().map(|word| weight(word)).sum();
    if total <= 0.0 {
        return Some(0);
    }
    let position = (elapsed / duration) * total;
    let mut cumulative = 0.0;
    for (index, word) in words.iter().enumerate() {
        cumulative += weight(word);
        if position < cumulative {
            return Some(index);
        }
    }
    Some(words.len() - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_of_reading_is_first_word() {
        assert_eq!(word_index("Hello world today", 0.0, 10.0), Some(0));
    }

    #[test]
    fn even_words_split_time_evenly() {
        assert_eq!(word_index("aaa bbb ccc", 5.0, 12.0), Some(1));
        assert_eq!(word_index("aaa bbb ccc", 2.0, 12.0), Some(0));
        assert_eq!(word_index("aaa bbb ccc", 11.0, 12.0), Some(2));
    }

    #[test]
    fn end_of_reading_is_last_word() {
        assert_eq!(word_index("aaa bbb ccc", 12.0, 12.0), Some(2));
    }

    #[test]
    fn past_the_end_is_none() {
        assert_eq!(word_index("aaa bbb ccc", 12.5, 12.0), None);
    }

    #[test]
    fn no_words_is_none() {
        assert_eq!(word_index("", 1.0, 10.0), None);
        assert_eq!(word_index("   ", 1.0, 10.0), None);
    }

    #[test]
    fn bad_times_are_none() {
        assert_eq!(word_index("aaa bbb ccc", 1.0, 0.0), None);
        assert_eq!(word_index("aaa bbb ccc", -1.0, 10.0), None);
        assert_eq!(word_index("aaa bbb ccc", f64::NAN, 10.0), None);
        assert_eq!(word_index("aaa bbb ccc", 1.0, f64::NAN), None);
    }

    #[test]
    fn punctuation_slows_a_word_down() {
        assert_eq!(word_index("Hi! Bye", 4.2, 8.5), Some(0));
        assert_eq!(word_index("Hi Bye", 4.2, 8.0), Some(1));
    }

    #[test]
    fn ranges_skip_extra_whitespace() {
        let text = "  one   two\nthree\t four  ";
        let ranges = word_ranges(text);
        let words: Vec<&str> = ranges.iter().map(|range| &text[range.clone()]).collect();
        assert_eq!(words, ["one", "two", "three", "four"]);
    }

    #[test]
    fn ranges_slice_persian_words() {
        let text = "سلام  دنیا";
        let ranges = word_ranges(text);
        assert_eq!(ranges.len(), 2);
        assert_eq!(&text[ranges[0].clone()], "سلام");
        assert_eq!(&text[ranges[1].clone()], "دنیا");
    }
}
