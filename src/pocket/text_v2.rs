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
use super::text::PieceEnd;

/// Highest token budget per v2 chunk (reference `_synth_unit`).
pub(crate) const MAX_TOKENS: usize = 18;

/// Lowest token budget per v2 chunk (reference `_synth_unit`).
pub(crate) const MIN_TOKENS: usize = 8;

/// Homo-GE2PE marks the ezafe with a trailing "1" (reference `synthesize`).
const EZAFE_MARK: char = '1';

/// Phoneme spellings (ezafe stripped, exact and case-sensitive) after
/// which a word cut is never allowed: conjunctions, prepositions,
/// determiners and similar words that bind to what follows.
const NO_CUT_AFTER: &[&str] = &[
    "va", "?o", "ke", "dar", "be", "?az", "bA", "tA", "bar", "barAye", "yA", "?in", "?An", "yek",
    "har",
];

/// Phoneme spellings (ezafe stripped, exact and case-sensitive) before
/// which a word cut is never allowed: words that bind to what precedes.
const NO_CUT_BEFORE: &[&str] = &["rA", "ro", "?ast"];

/// Phoneme spellings (ezafe stripped, exact and case-sensitive) before
/// which a word cut is preferred when a cut is needed: conjunctions and
/// clause openers.
const PREFER_CUT_BEFORE: &[&str] = &[
    "va", "ke", "?ammA", "vali", "yA", "tA", "Cun", "?agar", "zirA", "pas", "sepas",
];

/// True when a cut between `prev` and `next` is allowed: `prev` carries
/// no ezafe mark, its stripped spelling is not in [`NO_CUT_AFTER`], and
/// `next`'s stripped spelling is not in [`NO_CUT_BEFORE`].
fn is_allowed_cut(prev: &str, next: &str) -> bool {
    !prev.ends_with(EZAFE_MARK)
        && !NO_CUT_AFTER.contains(&strip_ezafe(prev).as_str())
        && !NO_CUT_BEFORE.contains(&strip_ezafe(next).as_str())
}

/// True when the cut before `words[at]` is allowed.
fn cut_allowed(words: &[&str], at: usize) -> bool {
    at > 0 && at < words.len() && is_allowed_cut(words[at - 1], words[at])
}

/// True when the cut before `words[at]` lands before a preferred word.
fn prefers_cut(words: &[&str], at: usize) -> bool {
    at < words.len() && PREFER_CUT_BEFORE.contains(&strip_ezafe(words[at]).as_str())
}

/// Best word segmentation of `words` into pieces within `max_tokens`
/// using only allowed cuts, or `None` when no such segmentation exists.
/// Best is, in order: fewest segments, fewest segments under
/// `min_tokens`, most cuts before a [`PREFER_CUT_BEFORE`] word, smallest
/// sum of squared (segment tokens − total/segments), then earliest cuts.
/// Token counts grow with segment length, so a segment is not extended
/// once it passes `max_tokens`.
fn search_word_segments(
    words: &[&str],
    count_tokens: &dyn Fn(&str) -> usize,
    max_tokens: usize,
    min_tokens: usize,
) -> Option<Vec<String>> {
    let n = words.len();
    if n == 0 {
        return Some(Vec::new());
    }
    // seg[i][j] = tokens in words[i..j], `None` once over the budget.
    let mut seg: Vec<Vec<Option<usize>>> = vec![vec![None; n + 1]; n];
    for i in 0..n {
        for j in (i + 1)..=n {
            let tokens = count_tokens(&words[i..j].join(" "));
            if tokens > max_tokens {
                break;
            }
            seg[i][j] = Some(tokens);
        }
    }
    const INF: usize = usize::MAX / 4;
    let mut dp_min = vec![INF; n + 1];
    dp_min[n] = 0;
    for i in (0..n).rev() {
        for j in (i + 1)..=n {
            if seg[i][j].is_none() || (j < n && !cut_allowed(words, j)) || dp_min[j] == INF {
                continue;
            }
            dp_min[i] = dp_min[i].min(1 + dp_min[j]);
        }
    }
    if dp_min[0] == INF {
        return None;
    }
    let total = count_tokens(&words.join(" ")) as f64;
    let average = total / dp_min[0] as f64;
    // Best suffix from each position among fewest-segment segmentations.
    #[derive(Clone)]
    struct Best {
        under: usize,
        prefer: usize,
        squared: f64,
        next: usize,
    }
    let mut best: Vec<Option<Best>> = vec![None; n + 1];
    best[n] = Some(Best {
        under: 0,
        prefer: 0,
        squared: 0.0,
        next: n,
    });
    for i in (0..n).rev() {
        let mut chosen: Option<Best> = None;
        for j in (i + 1)..=n {
            let Some(tokens) = seg[i][j] else { continue };
            if j < n && !cut_allowed(words, j) {
                continue;
            }
            if dp_min[j] == INF || 1 + dp_min[j] != dp_min[i] {
                continue;
            }
            let Some(suffix) = best[j].clone() else {
                continue;
            };
            let candidate = Best {
                under: usize::from(tokens < min_tokens) + suffix.under,
                prefer: usize::from(j < n && prefers_cut(words, j)) + suffix.prefer,
                squared: (tokens as f64 - average).powi(2) + suffix.squared,
                next: j,
            };
            let better = match &chosen {
                None => true,
                Some(cur) => {
                    candidate.under < cur.under
                        || (candidate.under == cur.under && candidate.prefer > cur.prefer)
                        || (candidate.under == cur.under
                            && candidate.prefer == cur.prefer
                            && candidate.squared < cur.squared - 1e-9)
                }
            };
            if better {
                chosen = Some(candidate);
            }
        }
        best[i] = chosen;
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        let next = best[i].as_ref()?.next;
        out.push(words[i..next].join(" "));
        i = next;
    }
    Some(out)
}

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
    text.split(is_v2_ws)
        .filter(|word| !word.is_empty())
        .collect()
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

/// Split one sentence into clauses on the soft breaks, keeping the mark
/// on the clause it ends (reference `SOFT_BREAK` split).
pub fn split_clauses(sentence: &str) -> Vec<String> {
    split_keep_all(sentence, &SOFT_BREAKS)
}

/// Plan one sentence's phoneme clauses into pieces: whole clauses merge
/// while they fit `max_tokens`; a clause over the budget is split with
/// [`split_chunks`] and never merges with its neighbours. A boundary at a
/// clause edge ends [`PieceEnd::Clause`], a boundary inside a split clause
/// ends [`PieceEnd::Mid`], and the last piece ends `sentence_end`. The
/// returned text keeps the ezafe marks.
pub(crate) fn plan_pieces(
    clauses: &[String],
    count_tokens: &dyn Fn(&str) -> usize,
    max_tokens: usize,
    min_tokens: usize,
    sentence_end: PieceEnd,
) -> Vec<(String, PieceEnd)> {
    let mut pieces: Vec<(String, PieceEnd)> = Vec::new();
    let mut current: Option<String> = None;
    for clause in clauses {
        if clause.trim().is_empty() {
            continue;
        }
        if count_tokens(clause) <= max_tokens {
            if let Some(cur) = current.as_ref() {
                let trial = format!("{cur} {clause}");
                if count_tokens(&trial) <= max_tokens {
                    current = Some(trial);
                    continue;
                }
                pieces.push((current.take().expect("just checked"), PieceEnd::Clause));
            }
            current = Some(clause.clone());
        } else {
            if let Some(cur) = current.take() {
                pieces.push((cur, PieceEnd::Clause));
            }
            let subs = split_chunks(clause, count_tokens, max_tokens, min_tokens);
            let last_sub = subs.len().saturating_sub(1);
            for (index, sub) in subs.into_iter().enumerate() {
                let end = if index == last_sub {
                    PieceEnd::Clause
                } else {
                    PieceEnd::Mid
                };
                pieces.push((sub, end));
            }
        }
    }
    if let Some(cur) = current.take() {
        pieces.push((cur, PieceEnd::Clause));
    }
    if let Some(last) = pieces.last_mut() {
        last.1 = sentence_end;
    }
    pieces
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
/// then words cut at sensible boundaries — a port of `split_text` in
/// `synthesize.py` with `keep_punct_boundaries=false` (the `_synth_unit`
/// call).
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
    let parts = split_recurse(&clean, count_tokens, max_tokens, min_tokens);
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
            let prev_words: Vec<String> = split_words(&merged[i - 1])
                .iter()
                .map(|s| s.to_string())
                .collect();
            if prev_words.len() < 2 {
                break;
            }
            let rest = prev_words[..prev_words.len() - 1].join(" ");
            if count_tokens(&rest) < min_tokens {
                break;
            }
            let moved = prev_words[prev_words.len() - 1].clone();
            // The new boundary sits before `moved`; it must be allowed.
            if !is_allowed_cut(&prev_words[prev_words.len() - 2], &moved) {
                break;
            }
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
    min_tokens: usize,
) -> Vec<String> {
    if count_tokens(piece) <= max_tokens {
        return vec![piece.to_string()];
    }
    for breaks in [HARD_BREAKS.as_slice(), SOFT_BREAKS.as_slice()] {
        let parts = split_keep_all(piece, breaks);
        if parts.len() > 1 {
            return parts
                .iter()
                .flat_map(|part| split_recurse(part, count_tokens, max_tokens, min_tokens))
                .collect();
        }
    }
    let words = split_words(piece);
    if let Some(segments) = search_word_segments(&words, count_tokens, max_tokens, min_tokens) {
        return segments;
    }
    let total = count_tokens(piece);
    let mut chunks = total.div_ceil(max_tokens).max(1);
    let mut packed: Vec<String> = Vec::new();
    for _ in 0..4 {
        packed = pack(
            &words,
            count_tokens,
            max_tokens,
            chunks,
            total.div_ceil(chunks),
        );
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
    stripped
        .chars()
        .last()
        .is_some_and(|c| BREAK_CHARS.contains(&c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_g2p_symbols_to_phones() {
        assert_eq!(
            translate_phonemes("ketabe1 j/dide1 m/n @/st"),
            "ketAbe1 jadide1 man ?ast"
        );
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
    fn split_clauses_keeps_soft_break_marks() {
        assert_eq!(
            split_clauses("سلام، دنیا؛ خداحافظ: رفت"),
            vec!["سلام،", "دنیا؛", "خداحافظ:", "رفت"]
        );
        assert_eq!(split_clauses("سلام دنیا"), vec!["سلام دنیا"]);
    }

    #[test]
    fn plan_pieces_merges_two_short_clauses() {
        let counter = |s: &str| s.split_whitespace().count();
        let clauses = vec!["a b".to_string(), "c d".to_string()];
        assert_eq!(
            plan_pieces(&clauses, &counter, 18, 8, PieceEnd::Sentence),
            vec![("a b c d".to_string(), PieceEnd::Sentence)]
        );
    }

    #[test]
    fn plan_pieces_three_clauses_end_clause_clause_sentence_end() {
        let counter = |s: &str| s.split_whitespace().count();
        let clause = |word: &str| [word; 10].join(" ");
        let clauses = vec![clause("a"), clause("b"), clause("c")];
        let pieces = plan_pieces(&clauses, &counter, 18, 8, PieceEnd::Sentence);
        assert_eq!(pieces.len(), 3, "{pieces:?}");
        assert_eq!(
            pieces.iter().map(|p| p.1).collect::<Vec<_>>(),
            vec![PieceEnd::Clause, PieceEnd::Clause, PieceEnd::Sentence]
        );
        // The sentence_end argument is used verbatim for the last piece.
        let pieces = plan_pieces(&clauses, &counter, 18, 8, PieceEnd::Mid);
        assert_eq!(pieces.last().map(|p| p.1), Some(PieceEnd::Mid));
    }

    #[test]
    fn plan_pieces_long_clause_between_short_ones() {
        let counter = |s: &str| s.split_whitespace().count();
        let long = ["w"; 30].join(" ");
        let clauses = vec!["a b".to_string(), long, "c d".to_string()];
        let pieces = plan_pieces(&clauses, &counter, 18, 8, PieceEnd::Sentence);
        let ends: Vec<PieceEnd> = pieces.iter().map(|p| p.1).collect();
        assert_eq!(pieces[0].0, "a b");
        assert_eq!(pieces.last().map(|p| p.0.as_str()), Some("c d"));
        // Clause edges end Clause, the inside of the split clause ends Mid.
        assert_eq!(ends[0], PieceEnd::Clause);
        assert!(ends[1..ends.len() - 1].contains(&PieceEnd::Mid), "{ends:?}");
        assert_eq!(
            ends[ends.len() - 2],
            PieceEnd::Clause,
            "last sub-piece of the long clause ends at its clause edge: {ends:?}"
        );
        assert_eq!(ends[ends.len() - 1], PieceEnd::Sentence);
        for piece in &pieces {
            assert!(counter(&piece.0) <= 18, "{}", piece.0);
        }
    }

    #[test]
    fn plan_pieces_empty_input_gives_no_pieces() {
        let counter = |s: &str| s.split_whitespace().count();
        assert!(plan_pieces(&[], &counter, 18, 8, PieceEnd::Sentence).is_empty());
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
    fn sensible_cut_prefers_before_va() {
        let counter = |s: &str| s.split_whitespace().count();
        assert_eq!(
            split_chunks("a b c va d e f", &counter, 4, 2),
            vec!["a b c", "va d e f"]
        );
    }

    #[test]
    fn sensible_cut_never_after_dar_or_before_ra() {
        let counter = |s: &str| s.split_whitespace().count();
        let chunks = split_chunks("a dar b c d e f g", &counter, 3, 2);
        for chunk in &chunks[..chunks.len() - 1] {
            assert_ne!(chunk.split_whitespace().last(), Some("dar"), "{chunks:?}");
        }
        let chunks = split_chunks("a b rA c d e f g", &counter, 3, 2);
        for chunk in &chunks[1..] {
            assert_ne!(chunk.split_whitespace().next(), Some("rA"), "{chunks:?}");
        }
    }

    #[test]
    fn sensible_cut_fifty_words_within_budget_and_minimum() {
        let counter = |s: &str| s.split_whitespace().count();
        let text = ["word"; 50].join(" ");
        let chunks = split_chunks(&text, &counter, 18, 8);
        assert!(chunks.len() >= 3, "{chunks:?}");
        for chunk in &chunks {
            let count = counter(chunk);
            assert!(count <= 18, "{chunk}");
            assert!(count >= 8, "{chunk}");
        }
    }

    #[test]
    fn short_tail_moves_words_back_unless_after_a_break() {
        let counter = |s: &str| s.split_whitespace().count();
        // Moving "c" back would cut after "b1", which carries an ezafe
        // mark, so that cut is not allowed and the short tail stays put.
        assert_eq!(
            split_chunks("a1 b1 c d", &counter, 3, 2),
            vec!["a1 b1 c", "d"]
        );
        // After a real break the short tail stays put.
        assert_eq!(
            split_chunks("w1 w2 w3. w4", &counter, 3, 2),
            vec!["w1 w2 w3.", "w4"]
        );
    }
}
