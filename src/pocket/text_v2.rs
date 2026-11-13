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
/// clause openers. `va` and `yA` are not listed here: they join phrases
/// as often as clauses (e.g. `SomAli va qarbi`), so a cut before them is
/// preferred only after a finite verb (see [`prefers_cut`]).
const PREFER_CUT_BEFORE: &[&str] = &[
    "ke", "?ammA", "vali", "tA", "Cun", "?agar", "zirA", "pas", "sepas",
];

/// Phoneme spellings (ezafe stripped, exact and case-sensitive) that are
/// finite verb forms on their own.
const FINITE_VERB_FORMS: &[&str] = &[
    "?ast", "nist", "hast", "bud", "Sod", "kard", "dAd", "goft", "raft", "?Amad",
];

/// True when `word` (ezafe stripped) looks like a finite verb: it starts
/// with `mi` or `nemi`, ends with `ad` or `and`, or is one of
/// [`FINITE_VERB_FORMS`].
fn looks_like_finite_verb(word: &str) -> bool {
    let word = strip_ezafe(word);
    word.starts_with("mi")
        || word.starts_with("nemi")
        || word.ends_with("ad")
        || word.ends_with("and")
        || FINITE_VERB_FORMS.contains(&word.as_str())
}

/// True when a cut between `prev` and `next` is allowed: `prev` carries
/// no ezafe mark, its stripped spelling is not in [`NO_CUT_AFTER`], and
/// `next`'s stripped spelling is not in [`NO_CUT_BEFORE`]. A cut is also
/// not allowed after a word whose stripped spelling ends in a vowel
/// followed by `ye` (`?ostAnhAye`, `hameye`, `sedAye`, `buye`): the model
/// often leaves the ezafe mark off such words, but the `ye` still binds
/// the word to what follows.
fn is_allowed_cut(prev: &str, next: &str) -> bool {
    let stripped = strip_ezafe(prev);
    let ends_vowel_ye = stripped.len() >= 3
        && stripped.ends_with("ye")
        && matches!(
            stripped[..stripped.len() - 2].chars().last(),
            Some('a' | 'e' | 'o' | 'A' | 'i' | 'u')
        );
    !prev.ends_with(EZAFE_MARK)
        && !ends_vowel_ye
        && !NO_CUT_AFTER.contains(&stripped.as_str())
        && !NO_CUT_BEFORE.contains(&strip_ezafe(next).as_str())
}

/// True when the cut before `words[at]` is allowed.
fn cut_allowed(words: &[&str], at: usize) -> bool {
    at > 0 && at < words.len() && is_allowed_cut(words[at - 1], words[at])
}

/// True when the cut before `words[at]` lands before a preferred word.
/// A cut before `va` or `yA` counts as preferred only when the previous
/// word looks like a finite verb ([`looks_like_finite_verb`]), because
/// those conjunctions join phrases as often as clauses.
fn prefers_cut(words: &[&str], at: usize) -> bool {
    if at == 0 || at >= words.len() {
        return false;
    }
    let next = strip_ezafe(words[at]);
    if PREFER_CUT_BEFORE.contains(&next.as_str()) {
        return true;
    }
    (next == "va" || next == "yA") && looks_like_finite_verb(words[at - 1])
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

/// ZWNJ (U+200C), inside Persian words such as «این‌قدر».
const ZWNJ: char = '\u{200c}';

/// Whitespace tokens of `text` that contain at least one Persian letter.
pub(crate) fn fa_words(text: &str) -> Vec<&str> {
    split_words(text)
        .into_iter()
        .filter(|word| word.chars().any(is_persian_letter))
        .collect()
}

fn zwnj_part_count(word: &str) -> usize {
    word.split(ZWNJ).count()
}

fn persian_letter_count(word: &str) -> usize {
    word.chars().filter(|&c| is_persian_letter(c)).count()
}

/// Drop repeated phoneme words the model emits for very short inputs.
/// `fa` is the sentence's Persian words. The phoneme list may keep at
/// most the total ZWNJ part count of `fa`; while it is longer and an
/// adjacent pair repeats, the second of the first such pair is dropped.
/// A lone phoneme word that is an exact doubling (`tamtam`) for a
/// Persian word of at most 3 letters is halved (`tam`).
pub(crate) fn dedupe_repeats(fa: &[&str], mut ph: Vec<String>) -> Vec<String> {
    let max = fa.iter().map(|word| zwnj_part_count(word)).sum::<usize>();
    while ph.len() > max {
        let Some(at) = (1..ph.len()).find(|&i| ph[i - 1] == ph[i]) else {
            break;
        };
        ph.remove(at);
    }
    if fa.len() == 1 && persian_letter_count(fa[0]) <= 3 && ph.len() == 1 {
        let word = &ph[0];
        let bytes_half = word.len() / 2;
        // Exact doubling of a char-boundary half at least 2 chars long.
        if word.len().is_multiple_of(2)
            && let (Some(half), Some(rest)) = (word.get(..bytes_half), word.get(bytes_half..))
            && half == rest
            && half.chars().count() >= 2
        {
            ph[0] = half.to_string();
        }
    }
    ph
}

/// For each Persian word in `fa`, how many phoneme words it owns: 1, or
/// its ZWNJ part count when that is more than 1. The counts must sum to
/// `ph.len()` exactly; among assignments that fit, the one minimising
/// the total |phoneme chars − 1.4 × Persian letters| wins, with ties
/// going to the earlier word taking 1. `None` when nothing fits.
pub(crate) fn align_words(fa: &[&str], ph: &[String]) -> Option<Vec<usize>> {
    let n = fa.len();
    let m = ph.len();
    let choices: Vec<Vec<usize>> = fa
        .iter()
        .map(|word| {
            let parts = zwnj_part_count(word);
            if parts > 1 { vec![1, parts] } else { vec![1] }
        })
        .collect();
    // dp[i][j]: best cost for fa[i..] owning ph[j..], with its choice.
    let mut dp: Vec<Vec<Option<(f64, usize)>>> = vec![vec![None; m + 1]; n + 1];
    dp[n][m] = Some((0.0, 0));
    for i in (0..n).rev() {
        for j in (0..=m).rev() {
            let mut best: Option<(f64, usize)> = None;
            for &count in &choices[i] {
                let end = j + count;
                if end > m {
                    continue;
                }
                let Some((suffix, _)) = dp[i + 1][end] else {
                    continue;
                };
                let joined_chars = ph[j..end].join(" ").chars().count() as f64;
                let cost = (joined_chars - 1.4 * persian_letter_count(fa[i]) as f64).abs() + suffix;
                // Strict `<` keeps the first (smallest) count on a tie.
                if best.is_none_or(|(seen, _)| cost < seen - 1e-9) {
                    best = Some((cost, count));
                }
            }
            dp[i][j] = best;
        }
    }
    dp[0][0]?;
    let mut counts = Vec::with_capacity(n);
    let mut j = 0usize;
    for row in dp.iter().take(n) {
        let (_, count) = row[j]?;
        counts.push(count);
        j += count;
    }
    Some(counts)
}

/// Join phoneme words into one string per Persian clause, where
/// `counts` (one per Persian word, in clause order) says how many
/// phoneme words each Persian word owns. A clause with no Persian word
/// contributes nothing.
pub(crate) fn clauses_from_counts(
    clauses_fa: &[String],
    ph: &[String],
    counts: &[usize],
) -> Vec<String> {
    let mut clauses = Vec::new();
    let mut at = 0usize;
    let mut word_at = 0usize;
    for clause in clauses_fa {
        let words = fa_words(clause);
        let owned: usize = counts[word_at..word_at + words.len()].iter().sum();
        word_at += words.len();
        if words.is_empty() {
            continue;
        }
        clauses.push(ph[at..at + owned].join(" "));
        at += owned;
    }
    clauses
}

/// Split phoneme words `ph` into one phoneme clause per Persian clause
/// in `clauses_fa`, using [`align_words`] over all their Persian words.
/// A clause with no Persian word contributes nothing. `None` when the
/// words cannot be aligned.
pub fn phoneme_clauses(clauses_fa: &[String], ph: &[String]) -> Option<Vec<String>> {
    let all: Vec<&str> = clauses_fa.iter().flat_map(|c| fa_words(c)).collect();
    let counts = align_words(&all, ph)?;
    Some(clauses_from_counts(clauses_fa, ph, &counts))
}

/// Replace the phoneme group of each Persian word that `lookup` knows
/// (by its spelling stripped of leading/trailing non-letter, non-ZWNJ
/// characters) with the override as one phoneme word. When the group's
/// last word carries the ezafe mark, the ezafe is kept: `ye1` after a
/// vowel-final override, `e1` otherwise. Returns the new phoneme words
/// and the new per-word counts (1 for a replaced word, unchanged on a
/// miss), which the caller uses to keep clause shares right.
pub(crate) fn apply_overrides(
    fa: &[&str],
    ph: &[String],
    counts: &[usize],
    lookup: &dyn Fn(&str) -> Option<String>,
) -> (Vec<String>, Vec<usize>) {
    let mut out: Vec<String> = Vec::with_capacity(ph.len());
    let mut new_counts: Vec<usize> = Vec::with_capacity(counts.len());
    let mut at = 0usize;
    for (word, &count) in fa.iter().zip(counts) {
        let group = &ph[at..at + count];
        at += count;
        let stripped = word.trim_matches(|c: char| !(is_persian_letter(c) || c == ZWNJ));
        let replaced = if stripped.is_empty() {
            None
        } else {
            lookup(stripped)
        };
        match replaced {
            Some(over) => {
                let has_ezafe = group.last().is_some_and(|last| last.ends_with(EZAFE_MARK));
                let word_out = if has_ezafe {
                    let vowel_final = over
                        .chars()
                        .last()
                        .is_some_and(|c| matches!(c, 'a' | 'e' | 'o' | 'A' | 'i' | 'u'));
                    if vowel_final {
                        format!("{over}ye1")
                    } else {
                        format!("{over}e1")
                    }
                } else {
                    over
                };
                out.push(word_out);
                new_counts.push(1);
            }
            None => {
                out.extend(group.iter().cloned());
                new_counts.push(count);
            }
        }
    }
    (out, new_counts)
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
    fn dedupe_repeats_drops_repeats_and_halves_doubling() {
        let words = |items: &[&str]| items.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            dedupe_repeats(&["سلام،"], words(&["salAm", "salAm"])),
            words(&["salAm"])
        );
        assert_eq!(
            dedupe_repeats(&["نه!"], words(&["na", "na"])),
            words(&["na"])
        );
        assert_eq!(
            dedupe_repeats(&["تم."], words(&["tamtam"])),
            words(&["tam"])
        );
        assert_eq!(
            dedupe_repeats(&["بابا"], words(&["bAbA"])),
            words(&["bAbA"])
        );
    }

    #[test]
    fn align_words_counts() {
        let words = |items: &[&str]| items.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            align_words(&["سلام", "دنیا"], &words(&["salAm", "donyA"])),
            Some(vec![1, 1])
        );
        assert_eq!(
            align_words(
                &["چرا", "این\u{200c}قدر", "دیر", "آمدی"],
                &words(&["CerA", "?in", "qadr", "dir", "?Amadi"])
            ),
            Some(vec![1, 2, 1, 1])
        );
        assert_eq!(
            align_words(
                &["لپ\u{200c}تاپ", "می\u{200c}خرم"],
                &words(&["lap", "tAp", "mixaram"])
            ),
            Some(vec![2, 1])
        );
        assert_eq!(
            align_words(
                &["لپ\u{200c}تاپ", "می\u{200c}خرم"],
                &words(&["laptAp", "mixaram"])
            ),
            Some(vec![1, 1])
        );
        assert_eq!(
            align_words(&["سلام", "دنیا"], &words(&["salAm", "donyA", "extra"])),
            None
        );
    }

    #[test]
    fn phoneme_clauses_splits_by_alignment() {
        let clauses = vec![
            "بعد از باران،".to_string(),
            "کفش\u{200c}هایم پر از گل شد.".to_string(),
        ];
        let ph: Vec<String> = [
            "ba?d", "?az", "bArAn", "kafSAyam", "por", "?az", "gel", "Sod",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            phoneme_clauses(&clauses, &ph),
            Some(vec![
                "ba?d ?az bArAn".to_string(),
                "kafSAyam por ?az gel Sod".to_string()
            ])
        );
    }

    #[test]
    fn apply_overrides_replaces_known_words_and_keeps_ezafe() {
        let words = |items: &[&str]| items.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let lookup = |word: &str| match word {
            "اکسل" => Some("?eksel".to_string()),
            "گوگل" => Some("gugel".to_string()),
            "دیتا" => Some("deytA".to_string()),
            "لپ\u{200c}تاپ" => Some("laptAp".to_string()),
            _ => None,
        };
        // A known word is replaced; its neighbour's ezafe is untouched.
        let (out, counts) = apply_overrides(
            &["فایل", "اکسل", "را"],
            &words(&["fAyle1", "?exkel", "rA"]),
            &[1, 1, 1],
            &lookup,
        );
        assert_eq!(out, words(&["fAyle1", "?eksel", "rA"]));
        assert_eq!(counts, vec![1, 1, 1]);
        // An ezafe on the replaced word is kept, `e1` after a consonant.
        let (out, _) = apply_overrides(
            &["گوگل", "جدید"],
            &words(&["gugle1", "jadid"]),
            &[1, 1],
            &lookup,
        );
        assert_eq!(out, words(&["gugele1", "jadid"]));
        // A vowel-final override gets `ye1`.
        let (out, _) = apply_overrides(
            &["دیتا", "جدید"],
            &words(&["deytA1", "jadid"]),
            &[1, 1],
            &lookup,
        );
        assert_eq!(out, words(&["deytAye1", "jadid"]));
        // A ZWNJ word the model split becomes its single override word.
        let (out, counts) =
            apply_overrides(&["لپ\u{200c}تاپ"], &words(&["lap", "tAp"]), &[2], &lookup);
        assert_eq!(out, words(&["laptAp"]));
        assert_eq!(counts, vec![1]);
        // Punctuation on the Persian word still matches; unknown words pass through.
        let (out, _) = apply_overrides(&["اکسل،"], &words(&["?exkel"]), &[1], &lookup);
        assert_eq!(out, words(&["?eksel"]));
        let (out, counts) = apply_overrides(&["سلام"], &words(&["salAm"]), &[1], &lookup);
        assert_eq!(out, words(&["salAm"]));
        assert_eq!(counts, vec![1]);
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
        // `miravad` ends with `ad`, so it looks like a finite verb and a
        // cut before the following `va` is preferred.
        assert_eq!(
            split_chunks("a b miravad va d e f", &counter, 4, 2),
            vec!["a b miravad", "va d e f"]
        );
    }

    #[test]
    fn sensible_cut_before_va_after_non_verb_gets_no_preference() {
        let counter = |s: &str| s.split_whitespace().count();
        // `SomAli` is not a finite verb, so the cut before `va` gets no
        // preference and the balanced cut (4/4, not 2/6) wins.
        assert_eq!(
            split_chunks("a SomAli va qarbi d e f g", &counter, 6, 2),
            vec!["a SomAli va qarbi", "d e f g"]
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
    fn sensible_cut_never_after_vowel_ye() {
        let counter = |s: &str| s.split_whitespace().count();
        // `?ostAnhAye` carries no ezafe mark, but its vowel + `ye` ending
        // still binds it to `SomAli`, so no piece may end with it.
        for word in ["?ostAnhAye", "hameye", "sedAye", "buye"] {
            assert!(!is_allowed_cut(word, "SomAli"), "{word}");
        }
        let chunks = split_chunks("a b ?ostAnhAye SomAli c d e f", &counter, 4, 2);
        for chunk in &chunks[..chunks.len() - 1] {
            assert_ne!(
                chunk.split_whitespace().last(),
                Some("?ostAnhAye"),
                "{chunks:?}"
            );
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
