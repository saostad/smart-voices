//! Per-chunk language routing: English chunks go to the Kokoro voice,
//! Persian (and other Arabic-script) chunks to the Persian Piper voice.

/// Chunk language, deciding which voice engine synthesizes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    English,
    Persian,
}

/// True for chars in the Arabic-script blocks (Arabic, supplements, and the
/// presentation forms).
fn is_arabic_script(c: char) -> bool {
    matches!(
        c,
        '\u{0600}'..='\u{06FF}'
            | '\u{0750}'..='\u{077F}'
            | '\u{08A0}'..='\u{08FF}'
            | '\u{FB50}'..='\u{FDFF}'
            | '\u{FE70}'..='\u{FEFF}'
    )
}

/// Detect a chunk's language: Persian when Arabic-script letters outnumber the
/// other alphabetic chars, else English. Pure Arabic text also routes to the
/// Persian voice; digits, punctuation and whitespace don't vote.
#[must_use]
pub fn chunk_language(text: &str) -> Lang {
    let mut arabic = 0u32;
    let mut other = 0u32;
    for c in text.chars() {
        if is_arabic_script(c) {
            arabic += 1;
        } else if c.is_alphabetic() {
            other += 1;
        }
    }
    if arabic > other {
        Lang::Persian
    } else {
        Lang::English
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_text_routes_to_english() {
        assert_eq!(chunk_language("Hello world."), Lang::English);
        assert_eq!(
            chunk_language("Dr. Smith said SQL is great!"),
            Lang::English
        );
    }

    #[test]
    fn persian_text_routes_to_persian() {
        assert_eq!(chunk_language("سلام دنیا."), Lang::Persian);
        assert_eq!(chunk_language("این یک متن فارسی است."), Lang::Persian);
    }

    #[test]
    fn mostly_persian_with_english_word_routes_to_persian() {
        assert_eq!(chunk_language("سلام دنیا Hello."), Lang::Persian);
    }

    #[test]
    fn mostly_english_with_persian_word_routes_to_english() {
        assert_eq!(chunk_language("Hello world سلام."), Lang::English);
    }

    #[test]
    fn digits_and_punctuation_only_route_to_english() {
        assert_eq!(chunk_language("123, 45.6!?"), Lang::English);
        assert_eq!(chunk_language("..."), Lang::English);
    }

    #[test]
    fn empty_text_routes_to_english() {
        assert_eq!(chunk_language(""), Lang::English);
    }
}
