//! Tiny Ava (Parisa) Persian TTS engine on ONNX Runtime.
//!
//! Runs `tiny-ava-parisa.onnx` (~21.4 MB, 24 kHz mono) using the shared
//! embedded `Homo-GE2PE` Persian G2P (`crate::pocket::g2p::G2p`), text
//! normalization (`crate::pocket::text_v2`), and gentle mastering
//! (`crate::pocket::audio::finish_speech_gentle`).

use std::borrow::Cow;
use std::path::Path;

use ort::session::{Session, SessionInputValue, SessionInputs};

use crate::engine::{EngineError, PersianVoice};
use crate::latin_fa::phoneme_overrides;
use crate::pocket::audio::{RawPiece, finish_speech_gentle};
use crate::pocket::g2p::G2p;
use crate::pocket::normalize_fa::is_persian_letter;
use crate::pocket::runtime::{extract_f32, ort_environment, tensor_f32, tensor_i64, thread_count};
use crate::pocket::text::{PieceEnd, classify_piece_end};
use crate::pocket::text_v2::{
    align_words, apply_overrides, clean_sentence_for_g2p, dedupe_repeats, fa_words, g2p_windows,
    normalize_for_model, split_clauses, split_sentences, translate_phonemes,
};

pub const SAMPLE_RATE: u32 = 24_000;
const MAX_TOKENS: usize = 512;
const LONG_SENTENCE_CHARS: usize = 350;
const G2P_WINDOW_CHARS: usize = 120;
const MIN_NEURAL_SPEED: f32 = 0.75;
const MAX_NEURAL_SPEED: f32 = 1.30;
const PITCH_SHIFT: f32 = 0.0;
const PITCH_RANGE: f32 = 1.05;
const PEAK_LIMIT: f32 = 0.99;

const PAD_ID: i64 = 0;
const BOS_ID: i64 = 1;
const EOS_ID: i64 = 2;
const SP_ID: i64 = 3;
const UNK_ID: i64 = 4;
const FIRST_PHONE_ID: i64 = 17;

/// Exact 46-symbol vocabulary from `tiny-ava-parisa.json`.
pub const SYMBOLS: [&str; 46] = [
    "<pad>", "<bos>", "<eos>", "<sp>", "<unk>", "،", ".", "؟", "!", "؛", ":", "«", "»", "(", ")",
    "—", "…", "p", "b", "t", "d", "k", "ɡ", "q", "ʔ", "f", "v", "s", "z", "ʃ", "ʒ", "x", "h", "tʃ",
    "dʒ", "m", "n", "l", "r", "j", "a", "e", "o", "i", "u", "ɑ",
];

fn punct_id(c: char) -> Option<i64> {
    match c {
        '،' => Some(5),
        '.' => Some(6),
        '؟' => Some(7),
        '!' => Some(8),
        '؛' => Some(9),
        ':' => Some(10),
        '«' => Some(11),
        '»' => Some(12),
        '(' => Some(13),
        ')' => Some(14),
        '—' => Some(15),
        '…' => Some(16),
        _ => None,
    }
}

fn repr1_char_to_id(c: char) -> Option<i64> {
    match c {
        'p' => Some(17),
        'b' => Some(18),
        't' => Some(19),
        'd' => Some(20),
        'k' => Some(21),
        'g' => Some(22),
        'q' => Some(23),
        '?' => Some(24),
        'f' => Some(25),
        'v' => Some(26),
        's' => Some(27),
        'z' => Some(28),
        'S' => Some(29),
        'Z' | ';' => Some(30),
        'x' => Some(31),
        'h' => Some(32),
        'C' => Some(33),
        'j' => Some(34),
        'm' => Some(35),
        'n' => Some(36),
        'l' => Some(37),
        'r' => Some(38),
        'y' => Some(39),
        'a' => Some(40),
        'e' => Some(41),
        'o' => Some(42),
        'i' => Some(43),
        'u' => Some(44),
        'A' => Some(45),
        _ => None,
    }
}

fn is_vowel_id(id: i64) -> bool {
    (40..=45).contains(&id)
}

/// Split clamped playback `speed` into pitch-preserving ONNX duration scaling
/// (`0.75..=1.30`) and residual waveform resampling (`1.0` inside the sweet spot).
pub fn decompose_speed(speed: f32) -> (f32, f32) {
    let clamped = if speed.is_finite() {
        speed.clamp(0.5, 2.0)
    } else {
        1.0
    };
    let neural = clamped.clamp(MIN_NEURAL_SPEED, MAX_NEURAL_SPEED);
    let resample = clamped / neural;
    (neural, resample)
}

fn is_repr1_vowel(b: u8) -> bool {
    matches!(b, b'a' | b'A' | b'e' | b'o' | b'i' | b'u')
}

/// Unstressed Persian conjunctions, prepositions, postpositions, enclitics, and copulas.
const UNSTRESSED_CLITICS: &[&str] = &[
    "va", "o", "ke", "dar", "be", "az", "bA", "tA", "yA", "bar", "rA", "ro", "ast", "am", "im",
    "id", "and", "ham", "niz",
];

const FUTURE_AUX: &[&str] = &["xAham", "xAhi", "xAhad", "xAhim", "xAhid", "xAhand"];

const LIGHT_PAST_STEMS: &[&str] = &[
    "kard", "Sod", "dAd", "gereft", "dASt", "zad", "Avard", "Avord", "bord", "yAft", "nemud",
    "sAxt", "gozASt", "xord", "bud", "gaSt", "gardid", "Amad",
];

const LIGHT_PRES_STEMS: &[&str] = &[
    "kon", "Sav", "dah", "deh", "gir", "dAr", "zan", "Avar", "bar", "yAb", "namA", "namAy", "sAz",
    "gozAr", "xor", "bAS", "gard", "Ay",
];

const VERB_PAST_STEMS: &[&str] = &[
    // Light-verb past stems
    "kard",
    "Sod",
    "dAd",
    "gereft",
    "dASt",
    "zad",
    "Avard",
    "Avord",
    "bord",
    "yAft",
    "nemud",
    "sAxt",
    "gozASt",
    "xord",
    "bud",
    "gaSt",
    "gardid",
    "Amad",
    // Common Persian news & prose past stems
    "goft",
    "raft",
    "tavAnest",
    "dAnest",
    "xAst",
    "did",
    "xAnd",
    "mAnd",
    "gozaSt",
    "neveSt",
    "ferestAd",
    "pardAxt",
    "andAxt",
    "afzud",
    "kAst",
    "neSast",
    "istAd",
    "oftAd",
    "koSt",
    "bast",
    "Sekast",
    "foruxt",
    "Amuxt",
    "suxt",
    "SenAxt",
    "paziroft",
    "paziraft",
    "peymud",
    "namud",
    "zist",
    "resid",
    "kaSid",
    "fahmid",
    "porsid",
    "xarid",
    "Sanid",
    "puSid",
    "jangid",
    "koSid",
    "tarsid",
    "baxSid",
    "talabid",
    "xAbid",
    "yAmad",
    "yAvard",
    "yAvord",
    "bardASt",
    "barAmad",
    "darAmad",
    "bAzgaSt",
    "dargozaSt",
    "barxAst",
    "farAgereft",
    "vAdASt",
];

const VERB_PRES_STEMS: &[&str] = &[
    // Light-verb present stems
    "kon", "Sav", "dah", "deh", "gir", "dAr", "zan", "Avar", "bar", "yAb", "namA", "namAy", "sAz",
    "gozAr", "xor", "bAS", "gard", "Ay", // Common Persian news & prose present stems
    "gu", "guy", "rav", "ro", "tavAn", "dAn", "xAh", "bin", "xAn", "mAn", "gozar", "nevis",
    "ferest", "pardAz", "andAz", "afzA", "afzAy", "kAh", "neSin", "ist", "oft", "koS", "band",
    "Sekan", "foruS", "Amuz", "suz", "SenAs", "pazir", "peymA", "peymAy", "res", "keS", "kaS",
    "fahm", "pors", "xar", "Senav", "puS", "jang", "kuS", "tars", "baxS", "talab", "xAb", "yAy",
    "yAvar", "yandAz",
];

const FINITE_ENDS: &[&str] = &["ad", "and", "am", "im", "id"];
const PERS_ENDS: &[&str] = &["and", "am", "im", "id"];
const PERFECT_ENDS: &[&str] = &[
    "e?and", "eand", "e?im", "eim", "e?id", "eid", "e?am", "eam", "e?i",
];
const PAST_PART_OR_PERS_ENDS: &[&str] = &[
    "", "e", "am", "im", "id", "and", "e?am", "eam", "e?i", "ei", "e?im", "eim", "e?id", "eid",
    "e?and", "eand",
];

/// Sentence-level state carried across words within one utterance so
/// affirmative light verbs following a stressed non-ezafe content word can
/// be de-accented (`stress = 0`), while resetting on punctuation or clitics.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct StressState {
    can_deaccent_light_verb: bool,
}

impl StressState {
    pub(crate) fn on_punct(&mut self) {
        self.can_deaccent_light_verb = false;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StressMode {
    None,
    FirstVowel,
    PenultVowel,
    LastVowel,
}

fn is_future_aux(canon: &str) -> bool {
    FUTURE_AUX.contains(&canon)
}

fn is_negative_future_aux(canon: &str) -> bool {
    canon.strip_prefix("na").is_some_and(is_future_aux)
}

fn is_light_past_participle(canon: &str) -> bool {
    canon
        .strip_suffix('e')
        .is_some_and(|stem| LIGHT_PAST_STEMS.contains(&stem))
}

fn is_affirmative_light_verb(canon: &str) -> bool {
    if is_future_aux(canon) {
        return true;
    }
    let (has_mi, past_rest) = if let Some(rest) = canon.strip_prefix("mi") {
        (true, rest)
    } else {
        (false, canon)
    };
    for stem in LIGHT_PAST_STEMS {
        if let Some(tail) = past_rest.strip_prefix(stem)
            && (PAST_PART_OR_PERS_ENDS.contains(&tail) || (has_mi && tail == "i"))
        {
            return true;
        }
    }
    if let Some(pres_rest) = canon
        .strip_prefix("mi")
        .or_else(|| canon.strip_prefix("be"))
        .or_else(|| canon.strip_prefix("bo"))
        .or_else(|| canon.starts_with("biy").then(|| &canon[2..]))
    {
        for stem in LIGHT_PRES_STEMS {
            if let Some(tail) = pres_rest.strip_prefix(stem)
                && matches!(tail, "am" | "i" | "ad" | "im" | "id" | "and")
            {
                return true;
            }
        }
    }
    // Bare present subjunctive light verb (never allow "-i" here so compound
    // action nouns like "tasmim giri" or "pAk sAzi" stay accented).
    for stem in LIGHT_PRES_STEMS {
        if let Some(tail) = canon.strip_prefix(stem)
            && matches!(tail, "am" | "ad" | "im" | "id" | "and")
        {
            return true;
        }
    }
    false
}

fn is_prefixed_finite_verb(canon: &str) -> bool {
    if canon.len() >= 6
        && let Some(rest) = canon.strip_prefix("nemi")
    {
        return FINITE_ENDS.iter().any(|e| rest.ends_with(e))
            || VERB_PAST_STEMS
                .iter()
                .any(|s| rest == *s || rest.strip_prefix(s) == Some("i"))
            || VERB_PRES_STEMS
                .iter()
                .any(|s| rest.strip_prefix(s) == Some("i"));
    }
    if canon.len() >= 5
        && let Some(rest) = canon.strip_prefix("mi")
    {
        // Exclude bare "-i" except explicit verb stems so news adjectives like
        // "milAdi", "milyArdi", "milyuni", "mihani", "mirAsi" keep final stress.
        return FINITE_ENDS.iter().any(|e| rest.ends_with(e))
            || VERB_PAST_STEMS
                .iter()
                .any(|s| rest == *s || rest.strip_prefix(s) == Some("i"))
            || VERB_PRES_STEMS
                .iter()
                .any(|s| rest.strip_prefix(s) == Some("i"));
    }
    if canon.len() >= 4
        && let Some(rest) = canon
            .strip_prefix("na")
            .or_else(|| canon.strip_prefix("ne"))
    {
        return VERB_PAST_STEMS.iter().any(|s| {
            rest.strip_prefix(s)
                .is_some_and(|t| PAST_PART_OR_PERS_ENDS.contains(&t) || t == "i")
        }) || VERB_PRES_STEMS.iter().any(|s| {
            rest.strip_prefix(s)
                .is_some_and(|t| matches!(t, "" | "am" | "i" | "ad" | "im" | "id" | "and"))
        });
    }
    if canon.len() >= 4
        && let Some(rest) = canon
            .strip_prefix("be")
            .or_else(|| canon.strip_prefix("bo"))
    {
        return VERB_PRES_STEMS.iter().any(|s| {
            rest.strip_prefix(s)
                .is_some_and(|t| matches!(t, "" | "am" | "i" | "ad" | "im" | "id" | "and"))
        });
    }
    false
}

fn has_unstressed_verbal_ending(canon: &str) -> bool {
    if canon.len() >= 5 && PERFECT_ENDS.iter().any(|e| canon.ends_with(e)) {
        return true;
    }
    for end in PERS_ENDS {
        if let Some(base) = canon.strip_suffix(end)
            && (VERB_PAST_STEMS.contains(&base) || (base.len() >= 4 && base.ends_with("id")))
        {
            return true;
        }
    }
    false
}

fn classify_stress(canon: &str, has_ezafe: bool, state: &mut StressState) -> StressMode {
    if has_ezafe {
        state.can_deaccent_light_verb = false;
        return StressMode::LastVowel;
    }
    if UNSTRESSED_CLITICS.contains(&canon) {
        state.can_deaccent_light_verb = false;
        return StressMode::None;
    }
    if state.can_deaccent_light_verb && is_affirmative_light_verb(canon) {
        state.can_deaccent_light_verb = is_future_aux(canon) || is_light_past_participle(canon);
        return StressMode::None;
    }
    if is_future_aux(canon) {
        state.can_deaccent_light_verb = true;
        return StressMode::PenultVowel;
    }
    if is_prefixed_finite_verb(canon) {
        state.can_deaccent_light_verb = is_negative_future_aux(canon)
            || ((canon.starts_with("na") || canon.starts_with("ne")) && canon.ends_with('e'));
        return StressMode::FirstVowel;
    }
    if has_unstressed_verbal_ending(canon) {
        state.can_deaccent_light_verb = false;
        return StressMode::PenultVowel;
    }
    state.can_deaccent_light_verb = true;
    StressMode::LastVowel
}

/// Convert one G2P phoneme word (from `Homo-GE2PE` with trailing `1` ezafe or
/// `persian-g2p-tiny` with `-e`/`-ye`, plus `-hAye` and `barAye` ezafe where
/// `Homo-GE2PE` drops the trailing `1`) into `(symbol_id, stress)` pairs using
/// Persian syntactic stress rules in `state`.
pub(crate) fn ge2pe_word_to_ipa_in_context(raw: &str, state: &mut StressState) -> Vec<(i64, i64)> {
    let (raw_stem, ez): (String, &str) = if raw.len() > 3 && raw.ends_with("ye1") {
        (raw[..raw.len() - 3].to_string(), "ye")
    } else if raw.len() > 2 && raw.ends_with("e1") {
        (raw[..raw.len() - 2].to_string(), "e")
    } else if raw.len() > 3 && raw.ends_with("-ye") {
        (raw[..raw.len() - 3].to_string(), "ye")
    } else if raw.len() > 2 && raw.ends_with("-e") {
        (raw[..raw.len() - 2].to_string(), "e")
    } else if raw.len() > 4 && raw.ends_with("hAye") {
        (raw[..raw.len() - 2].to_string(), "ye")
    } else if raw == "barAye" {
        ("barA".to_string(), "ye")
    } else {
        (raw.replace('1', ""), "")
    };

    let mut stem = raw_stem.replace('-', "");
    let bytes = stem.as_bytes();
    if bytes.len() >= 2 && bytes[0] == b'?' && is_repr1_vowel(bytes[1]) {
        stem.remove(0);
    }

    let canon = stem.replace("sh", "S").replace("zh", "Z");
    let mode = classify_stress(&canon, !ez.is_empty(), state);

    let mut phones: Vec<(i64, i64)> = Vec::with_capacity(stem.len() + ez.len());
    let chars: Vec<char> = stem.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if i + 1 < chars.len() && chars[i] == 'z' && chars[i + 1] == 'h' {
            phones.push((30, 0)); // ʒ
            i += 2;
            continue;
        }
        if i + 1 < chars.len() && chars[i] == 's' && chars[i + 1] == 'h' {
            phones.push((29, 0)); // ʃ
            i += 2;
            continue;
        }
        if let Some(id) = repr1_char_to_id(chars[i]) {
            phones.push((id, 0));
        }
        i += 1;
    }

    let vowel_idxs: Vec<usize> = phones
        .iter()
        .enumerate()
        .filter_map(|(idx, (id, _))| is_vowel_id(*id).then_some(idx))
        .collect();
    let target_vowel = match mode {
        StressMode::None => None,
        StressMode::FirstVowel => vowel_idxs.first().copied(),
        StressMode::PenultVowel => {
            if vowel_idxs.len() >= 2 {
                vowel_idxs.get(vowel_idxs.len() - 2).copied()
            } else {
                vowel_idxs.last().copied()
            }
        }
        StressMode::LastVowel => vowel_idxs.last().copied(),
    };
    if let Some(idx) = target_vowel {
        phones[idx].1 = 1;
    }

    // Explicit ezafe carries stress=2 on its vowel.
    for ch in ez.chars() {
        if let Some(id) = repr1_char_to_id(ch) {
            let lvl = if is_vowel_id(id) { 2 } else { 0 };
            phones.push((id, lvl));
        }
    }

    phones
}

/// Convert one isolated G2P phoneme word with fresh sentence state.
#[cfg(test)]
pub(crate) fn ge2pe_word_to_ipa(raw: &str) -> Vec<(i64, i64)> {
    ge2pe_word_to_ipa_in_context(raw, &mut StressState::default())
}

#[derive(Debug, Clone)]
pub(crate) enum TokenItem {
    Punct(i64),
    Word(Vec<(i64, i64)>),
}

pub(crate) fn ensure_terminal_mark(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if matches!(
        trimmed.chars().last(),
        Some('.' | '!' | '؟' | '?' | '…' | '،' | ',' | ';' | '؛' | ':')
    ) {
        trimmed.to_string()
    } else {
        format!("{trimmed}.")
    }
}

pub(crate) fn split_punct_chunks(text: &str) -> Vec<(bool, String)> {
    let mut out = Vec::new();
    let mut start = 0usize;
    for (idx, ch) in text.char_indices() {
        if punct_id(ch).is_none() {
            continue;
        }
        let seg = text[start..idx].trim();
        if !seg.is_empty() {
            out.push((false, seg.to_string()));
        }
        out.push((true, ch.to_string()));
        start = idx + ch.len_utf8();
    }
    let tail = text[start..].trim();
    if !tail.is_empty() {
        out.push((false, tail.to_string()));
    }
    out
}

fn halve_to_budget(part: String, out: &mut Vec<String>) {
    if part.chars().count() <= LONG_SENTENCE_CHARS {
        out.push(part);
        return;
    }
    let words: Vec<&str> = part.split_whitespace().collect();
    if words.len() < 2 {
        out.push(part);
        return;
    }
    let half = words.len().div_ceil(2);
    halve_to_budget(words[..half].join(" "), out);
    halve_to_budget(words[half..].join(" "), out);
}

pub(crate) fn split_long_sentence(sentence: &str, final_end: PieceEnd) -> Vec<(String, PieceEnd)> {
    if sentence.chars().count() <= LONG_SENTENCE_CHARS {
        return vec![(sentence.to_string(), final_end)];
    }
    let clauses = split_clauses(sentence);
    let mut packed: Vec<String> = Vec::new();
    for clause in clauses {
        if let Some(cur) = packed.last_mut() {
            let trial_len = cur.chars().count() + 1 + clause.chars().count();
            if trial_len <= LONG_SENTENCE_CHARS {
                cur.push(' ');
                cur.push_str(&clause);
                continue;
            }
        }
        packed.push(clause);
    }
    let mut parts = Vec::with_capacity(packed.len());
    for part in packed {
        halve_to_budget(part, &mut parts);
    }
    if parts.is_empty() {
        return vec![(sentence.to_string(), final_end)];
    }
    let last = parts.len().saturating_sub(1);
    parts
        .into_iter()
        .enumerate()
        .map(|(i, part)| {
            let end = if i == last {
                final_end
            } else {
                classify_piece_end(&part)
            };
            (part, end)
        })
        .collect()
}

/// Map aligned phoneme words (`final_ph`, `counts`) back onto the original
/// normalized `window`, keeping commas, periods, and quotes in exact word order
/// while threading sentence-level `stress_state`.
pub(crate) fn attach_window_items(
    window: &str,
    final_ph: &[String],
    counts: &[usize],
    stress_state: &mut StressState,
) -> Vec<TokenItem> {
    let mut items = Vec::new();
    let mut fa_idx = 0usize;
    let mut ph_at = 0usize;
    for token in window.split_whitespace() {
        if let (Some(first_byte), Some((last_byte, last_ch))) = (
            token
                .char_indices()
                .find(|&(_, c)| is_persian_letter(c))
                .map(|(i, _)| i),
            token.char_indices().rfind(|&(_, c)| is_persian_letter(c)),
        ) {
            let last_end = last_byte + last_ch.len_utf8();
            for ch in token[..first_byte].chars() {
                if let Some(id) = punct_id(ch) {
                    stress_state.on_punct();
                    items.push(TokenItem::Punct(id));
                }
            }
            if let Some(&count) = counts.get(fa_idx) {
                let end = (ph_at + count).min(final_ph.len());
                for ph in &final_ph[ph_at..end] {
                    let phones = ge2pe_word_to_ipa_in_context(ph, stress_state);
                    if !phones.is_empty() {
                        items.push(TokenItem::Word(phones));
                    }
                }
                ph_at = end;
                fa_idx += 1;
            }
            for ch in token[last_end..].chars() {
                if let Some(id) = punct_id(ch) {
                    stress_state.on_punct();
                    items.push(TokenItem::Punct(id));
                }
            }
        } else {
            for ch in token.chars() {
                if let Some(id) = punct_id(ch) {
                    stress_state.on_punct();
                    items.push(TokenItem::Punct(id));
                }
            }
        }
    }
    items
}

pub(crate) fn build_token_sequence(items: &[TokenItem]) -> Option<(Vec<i64>, Vec<i64>)> {
    let mut tokens = vec![BOS_ID];
    let mut stress = vec![0i64];
    let mut has_phone = false;

    for item in items {
        match item {
            TokenItem::Punct(id) => {
                if tokens.last().copied() != Some(*id) {
                    tokens.push(*id);
                    stress.push(0);
                }
            }
            TokenItem::Word(phones) => {
                if phones.is_empty() {
                    continue;
                }
                if let Some(&prev) = tokens.last()
                    && (prev >= FIRST_PHONE_ID || prev == UNK_ID)
                {
                    tokens.push(SP_ID);
                    stress.push(0);
                }
                for &(ph_id, lvl) in phones {
                    tokens.push(ph_id);
                    stress.push(if ph_id == UNK_ID || ph_id == PAD_ID {
                        0
                    } else {
                        lvl
                    });
                    if ph_id >= FIRST_PHONE_ID {
                        has_phone = true;
                    }
                }
            }
        }
    }

    tokens.push(EOS_ID);
    stress.push(0);
    has_phone.then_some((tokens, stress))
}

fn split_items_to_budget(
    items: &[TokenItem],
    end: PieceEnd,
) -> Vec<(Vec<i64>, Vec<i64>, PieceEnd)> {
    let Some((tokens, stress)) = build_token_sequence(items) else {
        return Vec::new();
    };
    if tokens.len() <= MAX_TOKENS || items.len() <= 1 {
        return vec![(tokens, stress, end)];
    }
    let mid = items.len() / 2;
    let mut out = split_items_to_budget(&items[..mid], PieceEnd::Mid);
    out.extend(split_items_to_budget(&items[mid..], end));
    out
}

/// Loaded Tiny Ava (Parisa) Persian TTS engine on ONNX Runtime.
pub struct TinyAvaEngine {
    session: Session,
    g2p: G2p,
}

impl TinyAvaEngine {
    /// Load `tiny-ava-parisa.onnx` from `models_root/tiny-ava/` and the
    /// embedded `Homo-GE2PE` Persian G2P model.
    pub fn load(models_root: &Path) -> Result<Self, EngineError> {
        let dir = models_root.join(PersianVoice::TinyAva.dir_name());
        let file = dir.join(PersianVoice::TinyAva.model_file());
        if !file.exists() {
            return Err(EngineError::MissingModel(file));
        }
        let env = ort_environment()?;
        let session = Session::builder(env)
            .map_err(|err| EngineError::TinyAvaLoadFailed(dir.clone(), err.to_string()))?
            .with_intra_threads(thread_count().min(4))
            .map_err(|err| EngineError::TinyAvaLoadFailed(dir.clone(), err.to_string()))?
            .commit_from_file(&file)
            .map_err(|err| EngineError::TinyAvaLoadFailed(dir, err.to_string()))?;
        let g2p = G2p::load()?;
        Ok(Self { session, g2p })
    }

    /// Output sample rate in Hz (24000).
    pub fn sample_rate(&self) -> i32 {
        SAMPLE_RATE as i32
    }

    fn pronounce_unaligned_window(
        &mut self,
        window: &str,
        items: &mut Vec<TokenItem>,
        stress_state: &mut StressState,
    ) -> Result<(), EngineError> {
        for (is_punct, piece) in split_punct_chunks(window) {
            if is_punct {
                if let Some(id) = piece.chars().next().and_then(punct_id) {
                    stress_state.on_punct();
                    items.push(TokenItem::Punct(id));
                }
                continue;
            }
            let clean = clean_sentence_for_g2p(&piece);
            if clean.trim().is_empty() {
                continue;
            }
            let phonemes = self.g2p.phonemise(&clean)?;
            let translated = translate_phonemes(&phonemes);
            let fa = fa_words(&piece);
            let window_ph: Vec<String> =
                translated.split_whitespace().map(str::to_string).collect();
            let window_ph = dedupe_repeats(&fa, window_ph);
            let final_ph = if let Some(counts) = align_words(&fa, &window_ph) {
                let lookup = |word: &str| phoneme_overrides().get(word).cloned();
                apply_overrides(&fa, &window_ph, &counts, &lookup).0
            } else {
                window_ph
            };
            for word in final_ph {
                let phones = ge2pe_word_to_ipa_in_context(&word, stress_state);
                if !phones.is_empty() {
                    items.push(TokenItem::Word(phones));
                }
            }
        }
        Ok(())
    }

    fn pronouncable_items(&mut self, text: &str) -> Result<Vec<TokenItem>, EngineError> {
        let with_mark = ensure_terminal_mark(text);
        let mut items = Vec::new();
        let mut stress_state = StressState::default();
        for window in g2p_windows(&with_mark, G2P_WINDOW_CHARS) {
            let clean = clean_sentence_for_g2p(&window);
            if clean.trim().is_empty() {
                for (is_punct, piece) in split_punct_chunks(&window) {
                    if is_punct && let Some(id) = piece.chars().next().and_then(punct_id) {
                        stress_state.on_punct();
                        items.push(TokenItem::Punct(id));
                    }
                }
                continue;
            }
            let phonemes = self.g2p.phonemise(&clean)?;
            let translated = translate_phonemes(&phonemes);
            let fa = fa_words(&window);
            let window_ph: Vec<String> =
                translated.split_whitespace().map(str::to_string).collect();
            let window_ph = dedupe_repeats(&fa, window_ph);
            if let Some(counts) = align_words(&fa, &window_ph) {
                let lookup = |word: &str| phoneme_overrides().get(word).cloned();
                let (final_ph, new_counts) = apply_overrides(&fa, &window_ph, &counts, &lookup);
                items.extend(attach_window_items(
                    &window,
                    &final_ph,
                    &new_counts,
                    &mut stress_state,
                ));
            } else {
                self.pronounce_unaligned_window(&window, &mut items, &mut stress_state)?;
            }
        }
        Ok(items)
    }

    fn run_onnx(
        &mut self,
        tokens: &[i64],
        stress: &[i64],
        neural_speed: f32,
    ) -> Result<Vec<f32>, EngineError> {
        let n = tokens.len();
        if n > MAX_TOKENS {
            return Err(EngineError::PocketData(format!(
                "tiny-ava token length {n} exceeds {MAX_TOKENS}"
            )));
        }
        let inputs: Vec<(Cow<'_, str>, SessionInputValue<'_>)> = vec![
            (
                Cow::Borrowed("tokens"),
                tensor_i64(vec![1, n], tokens.to_vec())?.into(),
            ),
            (
                Cow::Borrowed("stress"),
                tensor_i64(vec![1, n], stress.to_vec())?.into(),
            ),
            (Cow::Borrowed("voice"), tensor_i64(vec![1], vec![0])?.into()),
            (
                Cow::Borrowed("speed"),
                tensor_f32(vec![1], vec![neural_speed])?.into(),
            ),
            (
                Cow::Borrowed("pitch_shift"),
                tensor_f32(vec![1], vec![PITCH_SHIFT])?.into(),
            ),
            (
                Cow::Borrowed("pitch_range"),
                tensor_f32(vec![1], vec![PITCH_RANGE])?.into(),
            ),
        ];
        let outputs = self
            .session
            .run(SessionInputs::<'_, '_, 0>::ValueMap(inputs))
            .map_err(|err| EngineError::PocketData(format!("tiny-ava step failed: {err}")))?;
        let mut wav = extract_f32(&outputs, "audio")?;
        let peak = wav.iter().fold(0.0f32, |acc, &x| acc.max(x.abs()));
        if peak > PEAK_LIMIT {
            let scale = PEAK_LIMIT / peak;
            for s in &mut wav {
                *s *= scale;
            }
        }
        Ok(wav)
    }

    /// Synthesize `text` at `speed`, returning gently mastered 24 kHz mono audio.
    pub fn synthesize(&mut self, text: &str, speed: f32) -> Result<Vec<f32>, EngineError> {
        let cleaned = text.replace('\0', "");
        let normalized =
            normalize_for_model(&cleaned).map_err(|_| EngineError::NoAudio(text.to_string()))?;
        let sentences = split_sentences(&normalized);
        if sentences.is_empty() {
            return Err(EngineError::NoAudio(text.to_string()));
        }
        let (neural_speed, resample_speed) = decompose_speed(speed);
        let mut raw_pieces = Vec::new();
        let last_sent = sentences.len().saturating_sub(1);

        for (sent_idx, sentence) in sentences.iter().enumerate() {
            let sent_end = if sent_idx == last_sent {
                classify_piece_end(&normalized)
            } else {
                PieceEnd::Sentence
            };
            for (piece_text, piece_end) in split_long_sentence(sentence, sent_end) {
                let items = self.pronouncable_items(&piece_text)?;
                for (toks, stress, end) in split_items_to_budget(&items, piece_end) {
                    let wav = self.run_onnx(&toks, &stress, neural_speed)?;
                    if !wav.is_empty() {
                        raw_pieces.push(RawPiece { audio: wav, end });
                    }
                }
            }
        }

        let audio = finish_speech_gentle(&raw_pieces, SAMPLE_RATE, resample_speed, None);
        if audio.is_empty() {
            return Err(EngineError::NoAudio(text.to_string()));
        }
        Ok(audio)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbols_table_matches_metadata() {
        assert_eq!(SYMBOLS.len(), 46);
        assert_eq!(SYMBOLS[0], "<pad>");
        assert_eq!(SYMBOLS[1], "<bos>");
        assert_eq!(SYMBOLS[2], "<eos>");
        assert_eq!(SYMBOLS[3], "<sp>");
        assert_eq!(SYMBOLS[4], "<unk>");
        assert_eq!(SYMBOLS[5], "،");
        assert_eq!(SYMBOLS[6], ".");
        assert_eq!(SYMBOLS[29], "ʃ");
        assert_eq!(SYMBOLS[30], "ʒ");
        assert_eq!(SYMBOLS[45], "ɑ");
        assert_eq!(punct_id('؟'), Some(7));
        assert_eq!(repr1_char_to_id(';'), Some(30));
    }

    #[test]
    fn ge2pe_ezafe_zh_sh_and_glottal_mapping() {
        // sAle1 -> s ɑ l(stress 1) e(stress 2)
        assert_eq!(
            ge2pe_word_to_ipa("sAle1"),
            vec![(27, 0), (45, 1), (37, 0), (41, 2)]
        );
        // natijeye1 -> n a t i dʒ e(stress 1) j e(stress 2)
        assert_eq!(
            ge2pe_word_to_ipa("natijeye1"),
            vec![
                (36, 0),
                (40, 0),
                (19, 0),
                (43, 0),
                (34, 0),
                (41, 1),
                (39, 0),
                (41, 2)
            ]
        );
        // ;Apon -> ʒ ɑ p o(stress 1) n
        assert_eq!(
            ge2pe_word_to_ipa(";Apon"),
            vec![(30, 0), (45, 0), (17, 0), (42, 1), (36, 0)]
        );
        // vizheye -> last vowel in stem (final 'e') gets stress 1; vizheye1 -> ezafe 'ye' gets stress 2
        assert_eq!(
            ge2pe_word_to_ipa("vizheye"),
            vec![(26, 0), (43, 0), (30, 0), (41, 0), (39, 0), (41, 1)]
        );
        assert_eq!(
            ge2pe_word_to_ipa("vizheye1"),
            vec![(26, 0), (43, 0), (30, 0), (41, 1), (39, 0), (41, 2)]
        );
        // mishavad -> prefix 'mi-' takes stress 1; final 'a' stays 0
        assert_eq!(
            ge2pe_word_to_ipa("mishavad"),
            vec![
                (35, 0),
                (43, 1),
                (29, 0),
                (40, 0),
                (26, 0),
                (40, 0),
                (20, 0)
            ]
        );
        // ?ast -> unstressed copula clitic -> all 0
        assert_eq!(ge2pe_word_to_ipa("?ast"), vec![(40, 0), (27, 0), (19, 0)]);
        // barAye -> barA(1) + ye(2)
        assert_eq!(
            ge2pe_word_to_ipa("barAye"),
            vec![(18, 0), (40, 0), (38, 0), (45, 1), (39, 0), (41, 2)]
        );
        // pahpAdhAye (plural -hA + ezafe where GE2PE omits '1') -> A gets stress 1, ezafe 'e' gets stress 2
        assert_eq!(
            ge2pe_word_to_ipa("pahpAdhAye"),
            vec![
                (17, 0),
                (40, 0),
                (32, 0),
                (17, 0),
                (45, 0),
                (20, 0),
                (32, 0),
                (45, 1),
                (39, 0),
                (41, 2),
            ]
        );
    }

    #[test]
    fn split_long_sentence_keeps_237_chars_whole_and_packs_over_350_chars() {
        let voa_237 = "این مجموعه آزمایش‌ها با عنوان شن‌های سیاه برگزار شده است و فرماندهی مرکزی ایالات متحده سنتکام، فرماندهی عملیات ویژه سنتکام، دفتر تحقیقات نیروی دریایی آمریکا، و شماری از نهادهای توسعه فناوری نظامی در آن مشارکت داشته‌اند.";
        let kept = split_long_sentence(voa_237, PieceEnd::Sentence);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].1, PieceEnd::Sentence);

        let over_350 = format!(
            "{voa_237} در ادامه، این نهادها گزارش دادند که همکاری مشترک، همچنان با سرعت بالایی ادامه پیدا می‌کند و نتایج آن به‌زودی منتشر خواهد شد، و همچنین بخش‌های دیگر این برنامه نیز در ماه‌های آینده گسترش می‌یابند."
        );
        assert!(over_350.chars().count() > LONG_SENTENCE_CHARS);
        let parts = split_long_sentence(&over_350, PieceEnd::Sentence);
        assert!(parts.len() >= 2);
        assert!(
            parts
                .iter()
                .all(|(p, _)| p.chars().count() <= LONG_SENTENCE_CHARS)
        );
        assert_eq!(parts.last().unwrap().1, PieceEnd::Sentence);
    }

    #[test]
    fn attach_window_items_keeps_punctuation_with_aligned_words() {
        let window = "سلام، دنیای زیبا.";
        let ph = vec![
            "salAm".to_string(),
            "donyAye1".to_string(),
            "zibA".to_string(),
        ];
        let mut state = StressState::default();
        let items = attach_window_items(window, &ph, &[1, 1, 1], &mut state);
        let (toks, _) = build_token_sequence(&items).unwrap();
        assert!(toks.contains(&5)); // ،
        assert!(toks.contains(&6)); // .
        // Must have <bos> ... <sp> between second and third word, and comma before second word
        assert_eq!(items.len(), 5);
    }

    #[test]
    fn syntactic_stress_clitics_and_ezafe_protection() {
        for clitic in ["va", "dar", "be", "?az", "rA", "ke", "?ast", "ham", "niz"] {
            let phones = ge2pe_word_to_ipa(clitic);
            assert!(
                phones.iter().all(|&(_, s)| s == 0),
                "{clitic} should be unstressed: {phones:?}"
            );
        }
        // Ezafe homonyms stay stressed on stem and take 2 on ezafe
        assert_eq!(
            ge2pe_word_to_ipa("dare1"),
            vec![(20, 0), (40, 1), (38, 0), (41, 2)]
        );
        assert_eq!(
            ge2pe_word_to_ipa("bare1"),
            vec![(18, 0), (40, 1), (38, 0), (41, 2)]
        );
        // Demonstratives / quantifiers stay stressed
        assert_eq!(ge2pe_word_to_ipa("in"), vec![(43, 1), (36, 0)]);
        assert_eq!(ge2pe_word_to_ipa("An"), vec![(45, 1), (36, 0)]);
        assert_eq!(ge2pe_word_to_ipa("yek"), vec![(39, 0), (41, 1), (21, 0)]);
    }

    #[test]
    fn syntactic_stress_prefixed_verbs_and_false_positive_guards() {
        let stresses =
            |w: &str| -> Vec<i64> { ge2pe_word_to_ipa(w).into_iter().map(|(_, s)| s).collect() };

        assert_eq!(stresses("miSavad"), vec![0, 1, 0, 0, 0, 0, 0]);
        assert_eq!(stresses("nemiSavad"), vec![0, 1, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(stresses("nakardand"), vec![0, 1, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(stresses("bekonad"), vec![0, 1, 0, 0, 0, 0, 0]);
        assert_eq!(stresses("naSode"), vec![0, 1, 0, 0, 0, 0]);

        for adj in [
            "milAdi", "milyArdi", "milyuni", "mihani", "boland", "bozorg", "navad", "namad",
            "nasim", "navid", "behbudi",
        ] {
            let phones = ge2pe_word_to_ipa(adj);
            let vowel_stresses: Vec<i64> = phones
                .iter()
                .filter(|&&(id, _)| is_vowel_id(id))
                .map(|&(_, s)| s)
                .collect();
            assert_eq!(
                vowel_stresses.last().copied(),
                Some(1),
                "{adj} should keep final-vowel stress: {phones:?}"
            );
            assert_eq!(
                vowel_stresses.iter().filter(|&&s| s == 1).count(),
                1,
                "{adj} should have single stress: {phones:?}"
            );
        }
    }

    #[test]
    fn syntactic_stress_past_and_perfect_endings() {
        let vowel_stresses = |w: &str| -> Vec<i64> {
            ge2pe_word_to_ipa(w)
                .into_iter()
                .filter(|&(id, _)| is_vowel_id(id))
                .map(|(_, s)| s)
                .collect()
        };
        assert_eq!(vowel_stresses("kardand"), vec![1, 0]);
        assert_eq!(vowel_stresses("dASte?and"), vec![0, 1, 0]);
        assert_eq!(vowel_stresses("residand"), vec![0, 1, 0]);
        assert_eq!(vowel_stresses("xAhad"), vec![1, 0]);
    }

    #[test]
    fn syntactic_stress_compound_verb_deaccenting_and_gerund_guard() {
        let word_stresses = |window: &str, ph: &[&str]| -> Vec<Vec<i64>> {
            let ph_owned: Vec<String> = ph.iter().map(|s| (*s).to_string()).collect();
            let counts = vec![1usize; ph.len()];
            let mut state = StressState::default();
            attach_window_items(window, &ph_owned, &counts, &mut state)
                .into_iter()
                .filter_map(|item| match item {
                    TokenItem::Word(phones) => Some(phones.into_iter().map(|(_, s)| s).collect()),
                    TokenItem::Punct(_) => None,
                })
                .collect()
        };

        let s1 = word_stresses("آزمایش کردند", &["?AzmAyeS", "kardand"]);
        assert_eq!(s1[0], vec![0, 0, 0, 0, 0, 1, 0]);
        assert!(s1[1].iter().all(|&s| s == 0), "{:?}", s1[1]);

        let s2 = word_stresses("صورت گرفته است", &["surat", "gerefte", "?ast"]);
        assert_eq!(s2[0], vec![0, 0, 0, 1, 0]);
        assert!(s2[1].iter().all(|&s| s == 0), "{:?}", s2[1]);
        assert!(s2[2].iter().all(|&s| s == 0), "{:?}", s2[2]);

        let s3 = word_stresses("برگزار شده است", &["bargozAr", "Sode", "?ast"]);
        assert!(s3[0].contains(&1));
        assert!(s3[1].iter().all(|&s| s == 0));
        assert!(s3[2].iter().all(|&s| s == 0));

        let s4 = word_stresses("منتشر خواهد شد", &["montaSer", "xAhad", "Sod"]);
        assert!(s4[0].contains(&1));
        assert!(s4[1].iter().all(|&s| s == 0), "{:?}", s4[1]);
        assert!(s4[2].iter().all(|&s| s == 0), "{:?}", s4[2]);

        let s5 = word_stresses("انجام داده شده است", &["anjAm", "dAde", "Sode", "?ast"]);
        assert!(s5[0].contains(&1));
        assert!(s5[1].iter().all(|&s| s == 0));
        assert!(s5[2].iter().all(|&s| s == 0));
        assert!(s5[3].iter().all(|&s| s == 0));

        // Negative light verb keeps prefix stress
        let s_neg = word_stresses("آزمایش نکردند", &["?AzmAyeS", "nakardand"]);
        assert_eq!(s_neg[1], vec![0, 1, 0, 0, 0, 0, 0, 0, 0]);

        // Postposition rA resets light-verb de-accenting
        let s_ra = word_stresses("کتاب را گرفتند", &["ketAb", "rA", "gereftand"]);
        assert!(s_ra[1].iter().all(|&s| s == 0));
        assert_eq!(s_ra[2], vec![0, 0, 0, 1, 0, 0, 0, 0, 0]);

        // Punctuation resets light-verb de-accenting
        let s_punct = word_stresses("خوب، کردند", &["xub", "kardand"]);
        assert_eq!(s_punct[1], vec![0, 1, 0, 0, 0, 0, 0]);

        // Compound action nouns / gerunds keep final-vowel stress
        let s_gerund1 = word_stresses("تصمیم گیری", &["tasmim", "giri"]);
        assert_eq!(s_gerund1[1], vec![0, 0, 0, 1]);
        let s_gerund2 = word_stresses("پاک سازی", &["pAk", "sAzi"]);
        assert_eq!(s_gerund2[1], vec![0, 0, 0, 1]);

        // Cross-window state stays intact across g2p_windows without intervening punct
        let mut state = StressState::default();
        let w1 = attach_window_items("آزمایش", &["?AzmAyeS".to_string()], &[1], &mut state);
        let w2 = attach_window_items("کردند.", &["kardand".to_string()], &[1], &mut state);
        assert!(matches!(&w1[0], TokenItem::Word(p) if p.iter().any(|&(_, s)| s == 1)));
        assert!(matches!(&w2[0], TokenItem::Word(p) if p.iter().all(|&(_, s)| s == 0)));
    }

    #[test]
    fn tokenize_inserts_spaces_and_preserves_punctuation() {
        let items = vec![
            TokenItem::Word(ge2pe_word_to_ipa("salAm")),
            TokenItem::Punct(5), // ،
            TokenItem::Word(ge2pe_word_to_ipa("hAle1")),
            TokenItem::Word(ge2pe_word_to_ipa("SomA")),
            TokenItem::Punct(7), // ؟
        ];
        let (toks, stress) = build_token_sequence(&items).unwrap();
        // <bos> s a l ɑ m ، h ɑ l e <sp> ʃ o m ɑ ؟ <eos>
        assert_eq!(
            toks,
            vec![
                1, 27, 40, 37, 45, 35, 5, 32, 45, 37, 41, 3, 29, 42, 35, 45, 7, 2
            ]
        );
        assert_eq!(
            stress,
            vec![0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 2, 0, 0, 0, 0, 1, 0, 0]
        );
    }

    #[test]
    fn decompose_speed_preserves_pitch_in_sweet_spot() {
        assert_eq!(decompose_speed(1.0), (1.0, 1.0));
        assert_eq!(decompose_speed(0.75), (0.75, 1.0));
        assert_eq!(decompose_speed(1.30), (1.30, 1.0));
        let (n_fast, r_fast) = decompose_speed(2.0);
        assert!((n_fast - 1.30).abs() < 1e-6);
        assert!((n_fast * r_fast - 2.0).abs() < 1e-5);
        let (n_slow, r_slow) = decompose_speed(0.5);
        assert!((n_slow - 0.75).abs() < 1e-6);
        assert!((n_slow * r_slow - 0.5).abs() < 1e-5);
        assert_eq!(decompose_speed(f32::NAN), (1.0, 1.0));
    }

    #[test]
    fn missing_model_returns_missing_model_error() {
        let dir = std::env::temp_dir().join(format!("sv_tiny_ava_missing_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let err = match TinyAvaEngine::load(&dir) {
            Ok(_) => panic!("expected missing model error"),
            Err(err) => err,
        };
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(
            matches!(err, EngineError::MissingModel(ref p) if p.ends_with("tiny-ava/tiny-ava-parisa.onnx")),
            "{err:?}"
        );
    }
}
