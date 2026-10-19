//! Pocket TTS text recipe: prompt prep, chunk splitting, piece-end
//! classification.

/// Prepare one chunk the way the reference runtime does before tokenizing:
/// flatten line breaks, then (unless the model flags say otherwise)
/// uppercase the first character and close a trailing word character with a
/// period. v1 capitalizes and punctuates; v2 speaks G2P phonemes as-is.
/// (The Farsi config disables space padding and semicolon removal, and
/// Persian has no case to change.)
pub(crate) fn prepare_text_prompt(
    chunk: &str,
    capitalize_first_letter: bool,
    append_terminal_punctuation: bool,
) -> String {
    let mut text = chunk.trim().replace(['\n', '\r'], " ");
    while text.contains("  ") {
        text = text.replace("  ", " ");
    }
    if text.is_empty() {
        return text;
    }
    if capitalize_first_letter {
        let mut chars = text.chars();
        if let Some(first) = chars.next() {
            let upper: String = first.to_uppercase().collect();
            text = format!("{upper}{}", chars.collect::<String>());
        }
    }
    if append_terminal_punctuation && text.chars().last().is_some_and(char::is_alphanumeric) {
        text.push('.');
    }
    text
}

const HARD_BREAKS: [char; 3] = ['.', '!', '؟'];
const SOFT_BREAKS: [char; 3] = ['،', '؛', ':'];

fn split_after(text: &str, breaks: &[char]) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < chars.len() {
        if breaks.contains(&chars[i]) && i + 1 < chars.len() && chars[i + 1].is_whitespace() {
            let piece: String = chars[start..=i].iter().collect();
            let piece = piece.trim().to_string();
            if !piece.is_empty() {
                parts.push(piece);
            }
            i += 1;
            while i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }
            start = i;
        } else {
            i += 1;
        }
    }
    if start < chars.len() {
        let piece: String = chars[start..].iter().collect();
        let piece = piece.trim().to_string();
        if !piece.is_empty() {
            parts.push(piece);
        }
    }
    if parts.len() > 1 { parts } else { Vec::new() }
}

/// Split normalized text into chunks of at most `max_tokens`, breaking at
/// sentence punctuation first, then clause punctuation, then words — a port
/// of `split_text` in the model's `farsi_tts.py` (with the default
/// `keep_punct_boundaries=false`, so neighbours merge back while they fit).
pub(crate) fn split_text(
    text: &str,
    count_tokens: &dyn Fn(&str) -> usize,
    max_tokens: usize,
) -> Vec<String> {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return Vec::new();
    }
    fn recurse(
        piece: &str,
        count_tokens: &dyn Fn(&str) -> usize,
        max_tokens: usize,
    ) -> Vec<String> {
        if count_tokens(piece) <= max_tokens {
            return vec![piece.to_string()];
        }
        for breaks in [HARD_BREAKS.as_slice(), SOFT_BREAKS.as_slice()] {
            let parts = split_after(piece, breaks);
            if !parts.is_empty() {
                return parts
                    .iter()
                    .flat_map(|part| recurse(part, count_tokens, max_tokens))
                    .collect();
            }
        }
        let mut out = Vec::new();
        let mut current = String::new();
        for word in piece.split_whitespace() {
            let trial = if current.is_empty() {
                word.to_string()
            } else {
                format!("{current} {word}")
            };
            if !current.is_empty() && count_tokens(&trial) > max_tokens {
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
    let parts = recurse(&text, count_tokens, max_tokens);
    let mut merged: Vec<String> = Vec::new();
    for chunk in parts {
        if let Some(last) = merged.last_mut() {
            let trial = format!("{last} {chunk}");
            if count_tokens(&trial) <= max_tokens {
                *last = trial;
                continue;
            }
        }
        merged.push(chunk);
    }
    merged
}

/// How the boundary after a piece sounds: a full stop keeps the old 120 ms
/// keeps and 150 ms pause, a clause mark keeps 60 ms and pauses 60 ms, and
/// a mid-sentence cut keeps 25 ms at each cut side with no pause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PieceEnd {
    Sentence,
    Clause,
    Mid,
}

/// Classify a piece's end by its last character: sentence punctuation (`.`,
/// `!`, `؟`, `?`), clause marks (`،`, `؛`, `:`, `,`, `;`), or a mid-sentence
/// cut (anything else, including empty text).
pub(crate) fn classify_piece_end(text: &str) -> PieceEnd {
    match text.chars().last() {
        Some('.') | Some('!') | Some('؟') | Some('?') => PieceEnd::Sentence,
        Some('،') | Some('؛') | Some(':') | Some(',') | Some(';') => PieceEnd::Clause,
        _ => PieceEnd::Mid,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_text_respects_token_budget_and_punctuation() {
        // One token per word in this toy counter.
        let counter = |s: &str| s.split_whitespace().count();
        let text = "یک دو سه چهار. پنج شش هفت هشت نه ده یازده دوازده سیزده چهارده پانزده شانزده هفده هجده نوزده بیست.";
        let chunks = split_text(text, &counter, 8);
        assert!(chunks.len() >= 3, "{chunks:?}");
        for chunk in &chunks {
            assert!(counter(chunk) <= 8, "{chunk}");
        }
        // Short text stays whole.
        assert_eq!(split_text("سلام دنیا.", &counter, 18), vec!["سلام دنیا."]);
        assert!(split_text("", &counter, 18).is_empty());
    }

    #[test]
    fn classify_piece_end_by_last_char() {
        assert_eq!(classify_piece_end("سلام."), PieceEnd::Sentence);
        assert_eq!(classify_piece_end("سلام!"), PieceEnd::Sentence);
        assert_eq!(classify_piece_end("سلام؟"), PieceEnd::Sentence);
        assert_eq!(classify_piece_end("salam?"), PieceEnd::Sentence);
        assert_eq!(classify_piece_end("سلام،"), PieceEnd::Clause);
        assert_eq!(classify_piece_end("سلام؛"), PieceEnd::Clause);
        assert_eq!(classify_piece_end("گفت:"), PieceEnd::Clause);
        assert_eq!(classify_piece_end("a,b"), PieceEnd::Mid);
        assert_eq!(classify_piece_end("a,"), PieceEnd::Clause);
        assert_eq!(classify_piece_end("a;"), PieceEnd::Clause);
        assert_eq!(classify_piece_end("نیمه"), PieceEnd::Mid);
        assert_eq!(classify_piece_end(""), PieceEnd::Mid);
    }
}
