//! Latin words inside Persian text, converted to Persian spelling so the
//! Persian voice reads them instead of dropping them.
//!
//! Pure: no I/O, no model. The word list is `assets/latin_fa.tsv`,
//! embedded at compile time; words it does not cover are spelled with
//! letter names or with approximate spelling rules.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::lang::is_arabic_script;

/// ZWNJ (U+200C), joining Persian letter names and word parts.
const ZWNJ: char = '\u{200c}';

/// Result of [`persianize`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Persianized {
    pub text: String,
    /// Latin words that fell through to the approximate rules (original spelling).
    pub approximated: Vec<String>,
    /// True when letters that are neither ASCII nor Arabic-script remain.
    pub skipped: bool,
}

fn word_list() -> &'static HashMap<String, String> {
    static LIST: OnceLock<HashMap<String, String>> = OnceLock::new();
    LIST.get_or_init(|| {
        let mut map = HashMap::new();
        for line in include_str!("../assets/latin_fa.tsv").lines() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut columns = line.split('\t');
            if let (Some(written), Some(persian)) = (columns.next(), columns.next()) {
                map.insert(written.to_string(), persian.to_string());
            }
        }
        map
    })
}

/// Map from the Persian spelling (second TSV column) to its exact
/// phonemes (third column), in the v2 model's alphabet. A row whose
/// phonemes column is empty has no override (the pronunciation model
/// decides), so it is skipped here.
pub(crate) fn phoneme_overrides() -> &'static HashMap<String, String> {
    static LIST: OnceLock<HashMap<String, String>> = OnceLock::new();
    LIST.get_or_init(|| {
        let mut map = HashMap::new();
        for line in include_str!("../assets/latin_fa.tsv").lines() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut columns = line.split('\t');
            if let (Some(_written), Some(persian), Some(phonemes)) =
                (columns.next(), columns.next(), columns.next())
            {
                if !phonemes.is_empty() {
                    map.insert(persian.to_string(), phonemes.to_string());
                }
            }
        }
        map
    })
}

fn letter_name(c: char) -> Option<&'static str> {
    Some(match c.to_ascii_uppercase() {
        'A' => "ای",
        'B' => "بی",
        'C' => "سی",
        'D' => "دی",
        'E' => "ای",
        'F' => "اف",
        'G' => "جی",
        'H' => "اچ",
        'I' => "آی",
        'J' => "جی",
        'K' => "کی",
        'L' => "ال",
        'M' => "ام",
        'N' => "ان",
        'O' => "او",
        'P' => "پی",
        'Q' => "کیو",
        'R' => "آر",
        'S' => "اس",
        'T' => "تی",
        'U' => "یو",
        'V' => "وی",
        'W' => "دابلیو",
        'X' => "ایکس",
        'Y' => "وای",
        'Z' => "زد",
        _ => return None,
    })
}

/// True for a consonant letter: not one of the vowels `a e i o u y`.
fn is_consonant(c: char) -> bool {
    c.is_ascii_alphabetic() && !matches!(c, 'a' | 'e' | 'i' | 'o' | 'u' | 'y')
}

/// Split a token into parts: a new part starts at a lower→upper change
/// and at the last capital of an upper-case run followed by a lower-case
/// letter (`ChatGPT` → `Chat`,`GPT`; `JSONParser` → `JSON`,`Parser`).
fn split_parts(token: &str) -> Vec<String> {
    let chars: Vec<char> = token.chars().collect();
    let mut parts = Vec::new();
    let mut start = 0usize;
    for i in 1..chars.len() {
        let prev = chars[i - 1];
        let cur = chars[i];
        let lower_to_upper = prev.is_ascii_lowercase() && cur.is_ascii_uppercase();
        let run_last_capital = prev.is_ascii_uppercase()
            && cur.is_ascii_uppercase()
            && chars.get(i + 1).is_some_and(char::is_ascii_lowercase);
        if lower_to_upper || run_last_capital {
            parts.push(chars[start..i].iter().collect());
            start = i;
        }
    }
    parts.push(chars[start..].iter().collect());
    parts
}

/// Letter rule: one letter, all upper-case with at most 5 letters, or no
/// `a e i o u y` in any case.
fn is_letter_spelled(part: &str) -> bool {
    let chars: Vec<char> = part.chars().collect();
    chars.len() == 1
        || (chars.iter().all(|c| c.is_ascii_uppercase()) && chars.len() <= 5)
        || !chars
            .iter()
            .any(|c| matches!(c.to_ascii_lowercase(), 'a' | 'e' | 'i' | 'o' | 'u' | 'y'))
}

fn spell_letters(part: &str) -> String {
    part.chars()
        .filter_map(letter_name)
        .collect::<Vec<_>>()
        .join(&ZWNJ.to_string())
}

/// Convert one part. Returns the Persian text and whether the
/// approximate spelling rules were used.
fn convert_part(part: &str) -> (String, bool) {
    if let Some(persian) = word_list().get(&part.to_lowercase()) {
        return (persian.clone(), false);
    }
    if is_letter_spelled(part) {
        return (spell_letters(part), false);
    }
    (spell_by_rules(&part.to_lowercase()), true)
}

/// Convert one token, first match wins: the word list for the whole
/// token, otherwise its parts joined with ZWNJ.
fn convert_token(token: &str, approximated: &mut Vec<String>) -> String {
    if let Some(persian) = word_list().get(&token.to_lowercase()) {
        return persian.clone();
    }
    let parts = split_parts(token);
    let mut used_rules = false;
    let mut converted = Vec::with_capacity(parts.len());
    for part in &parts {
        let (text, by_rules) = convert_part(part);
        used_rules |= by_rules;
        converted.push(text);
    }
    if used_rules && !approximated.iter().any(|word| word == token) {
        approximated.push(token.to_string());
    }
    converted.join(&ZWNJ.to_string())
}

/// Convert Latin-letter words in `text` to Persian spelling. Everything
/// else is copied as is.
#[must_use]
pub fn persianize(text: &str) -> Persianized {
    let chars: Vec<char> = text.chars().collect();
    let mut result = String::with_capacity(text.len());
    let mut approximated: Vec<String> = Vec::new();
    let mut index = 0usize;
    while index < chars.len() {
        if chars[index].is_ascii_alphabetic() {
            let mut token = String::new();
            while index < chars.len() {
                if chars[index].is_ascii_alphabetic() {
                    token.push(chars[index]);
                    index += 1;
                } else if matches!(chars[index], '\'' | '\u{2019}')
                    && !token.is_empty()
                    && chars
                        .get(index + 1)
                        .is_some_and(|c| c.is_ascii_alphabetic())
                {
                    // An apostrophe between two letters belongs to the
                    // token and is dropped.
                    index += 1;
                } else {
                    break;
                }
            }
            result.push_str(&convert_token(&token, &mut approximated));
            if chars.get(index).is_some_and(|&c| is_arabic_script(c)) {
                result.push(ZWNJ);
            }
        } else {
            result.push(chars[index]);
            index += 1;
        }
    }
    let skipped = result
        .chars()
        .any(|c| c.is_alphabetic() && !c.is_ascii() && !is_arabic_script(c));
    Persianized {
        text: result,
        approximated,
        skipped,
    }
}

fn consonant_text(word: &[char], at: usize) -> String {
    let c = word[at];
    match c {
        'b' => "ب".to_string(),
        'd' => "د".to_string(),
        'f' => "ف".to_string(),
        'h' => "ه".to_string(),
        'j' => "ج".to_string(),
        'k' => "ک".to_string(),
        'l' => "ل".to_string(),
        'm' => "م".to_string(),
        'n' => "ن".to_string(),
        'p' => "پ".to_string(),
        'q' => "ک".to_string(),
        'r' => "ر".to_string(),
        's' => "س".to_string(),
        't' => "ت".to_string(),
        'v' => "و".to_string(),
        'w' => "و".to_string(),
        'x' => "کس".to_string(),
        'z' => "ز".to_string(),
        'c' => {
            if word
                .get(at + 1)
                .is_some_and(|next| matches!(next, 'e' | 'i' | 'y'))
            {
                "س".to_string()
            } else {
                "ک".to_string()
            }
        }
        'g' => {
            // `g` is ج when followed by a word-final `e`, else گ.
            if word.get(at + 1) == Some(&'e') && at + 2 == word.len() {
                "ج".to_string()
            } else {
                "گ".to_string()
            }
        }
        'y' => "ی".to_string(),
        _ => String::new(),
    }
}

fn two_letter_text(pair: &str) -> Option<&'static str> {
    Some(match pair {
        "sh" => "ش",
        "ch" => "چ",
        "th" => "ت",
        "ph" => "ف",
        "ck" => "ک",
        "qu" => "کو",
        "wh" => "و",
        "gh" => "گ",
        "ee" | "ea" | "ie" | "ey" | "ai" | "ay" | "ei" => "ی",
        "oo" | "ou" | "ow" | "oa" => "و",
        "oi" | "oy" => "وی",
        "au" | "aw" => "ا",
        _ => return None,
    })
}

fn is_vowel_pair(pair: &str) -> bool {
    matches!(
        pair,
        "ee" | "ea"
            | "ie"
            | "ey"
            | "ai"
            | "ay"
            | "ei"
            | "oo"
            | "ou"
            | "ow"
            | "oa"
            | "oi"
            | "oy"
            | "au"
            | "aw"
    )
}

/// Approximate Persian spelling of one lower-cased part, left to right,
/// first match at each position. This is the rule function the tests
/// call directly, bypassing the word list.
fn spell_by_rules(word: &str) -> String {
    let chars: Vec<char> = word.chars().collect();
    let n = chars.len();
    let mut out = String::new();
    let mut i = 0usize;
    while i < n {
        let rest: String = chars[i..].iter().collect();
        // Start only: `s` followed by a consonant other than `h`.
        if i == 0 && chars[i] == 's' && n > 1 && is_consonant(chars[1]) && chars[1] != 'h' {
            out.push_str("اس");
            i += 1;
            continue;
        }
        // Endings: the rest of the word is exactly this.
        if rest == "tion" {
            out.push_str("شن");
            break;
        }
        if rest == "sion" {
            out.push_str("ژن");
            break;
        }
        if rest == "ture" {
            out.push_str("چر");
            break;
        }
        if rest == "er" {
            out.push('ر');
            break;
        }
        if rest == "or" {
            out.push_str("ور");
            break;
        }
        if rest == "le" && i > 0 && is_consonant(chars[i - 1]) {
            out.push('ل');
            break;
        }
        if rest == "e" && i > 0 && is_consonant(chars[i - 1]) && n > 2 {
            break;
        }
        if rest == "y" && i > 0 {
            out.push('ی');
            break;
        }
        // Two letters.
        if i + 1 < n {
            let pair: String = chars[i..i + 2].iter().collect();
            if let Some(text) = two_letter_text(&pair) {
                if i == 0 && is_vowel_pair(&pair) && !text.starts_with('ا') {
                    out.push('ا');
                }
                out.push_str(text);
                i += 2;
                continue;
            }
        }
        // A doubled consonant is written once.
        if i + 1 < n && chars[i] == chars[i + 1] && is_consonant(chars[i]) {
            out.push_str(&consonant_text(&chars, i));
            i += 2;
            continue;
        }
        let c = chars[i];
        if is_consonant(c) || c == 'y' && i == 0 {
            out.push_str(&consonant_text(&chars, i));
            i += 1;
            continue;
        }
        // Vowels.
        match (i == 0, i + 1 == n, c) {
            (true, _, 'a') => out.push('ا'),
            (true, _, 'e') => out.push('ا'),
            (true, _, 'i') => out.push_str("ای"),
            (true, _, 'o') => out.push('ا'),
            (true, _, 'u') => out.push('ا'),
            (_, true, 'a') => out.push('ا'),
            (_, true, 'i') => out.push('ی'),
            (_, true, 'o') => out.push('و'),
            (_, true, 'u') => out.push('و'),
            (_, true, 'e') => out.push('ی'),
            (_, _, 'a') => {
                if chars.get(i + 1) == Some(&'r') {
                    out.push('ا');
                } else if i + 2 < n
                    && is_consonant(chars[i + 1])
                    && chars[i + 2] == 'e'
                    && i + 3 == n
                {
                    // Followed by one consonant then a word-final `e`.
                    out.push('ی');
                } else if i + 2 < n
                    && is_consonant(chars[i + 1])
                    && matches!(chars[i + 2], 'a' | 'e' | 'i' | 'o' | 'u' | 'y')
                {
                    out.push('ا');
                }
            }
            (_, _, 'e') => {}
            (_, _, 'i' | 'y') => out.push('ی'),
            (_, _, 'o') => out.push('و'),
            (_, _, 'u') => out.push('ا'),
            _ => {}
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spelling_rules_only() {
        let cases = [
            ("chat", "چت"),
            ("smart", "اسمارت"),
            ("start", "استارت"),
            ("web", "وب"),
            ("test", "تست"),
            ("link", "لینک"),
            ("google", "گوگل"),
            ("server", "سرور"),
            ("bug", "باگ"),
            ("linux", "لیناکس"),
            ("java", "جاوا"),
            ("name", "نیم"),
            ("phone", "فون"),
            ("error", "ارور"),
            ("update", "اپدیت"),
            ("cluster", "کلاستر"),
            ("page", "پیج"),
            ("book", "بوک"),
            ("team", "تیم"),
            ("cloud", "کلود"),
            ("city", "سیتی"),
            ("each", "ایچ"),
        ];
        for (input, expected) in cases {
            assert_eq!(spell_by_rules(input), expected, "rule spelling of {input}");
        }
    }

    #[test]
    fn persianize_whole_text() {
        assert_eq!(persianize("فایل PDF را").text, "فایل پی‌دی‌اف را");
        assert_eq!(persianize("ChatGPT").text, "چت‌جی‌پی‌تی");
        assert_eq!(persianize("VS Code").text, "وی‌اس کد");
        assert_eq!(persianize("iPhone").text, "آیفون");
        assert_eq!(persianize("APIها").text, "ای‌پی‌آی‌ها");
    }

    #[test]
    fn persianize_leaves_persian_and_reports() {
        let plain = persianize("سلام دنیا، این یک متن فارسی است.");
        assert_eq!(plain.text, "سلام دنیا، این یک متن فارسی است.");
        assert!(plain.approximated.is_empty(), "{plain:?}");
        assert!(!plain.skipped, "{plain:?}");

        let skipped = persianize("سلام Привет");
        assert!(skipped.skipped, "{skipped:?}");

        // A word-list word is not approximated; a rules word is, once.
        let mixed = persianize("code cluster cluster");
        assert_eq!(mixed.text, "کد کلاستر کلاستر");
        assert_eq!(mixed.approximated, vec!["cluster".to_string()]);
    }

    #[test]
    fn phoneme_overrides_map_persian_to_phonemes() {
        let overrides = phoneme_overrides();
        assert_eq!(overrides.get("اکسل").map(String::as_str), Some("?eksel"));
        assert_eq!(overrides.get("گوگل").map(String::as_str), Some("gugel"));
        assert_eq!(overrides.get("فرم‌ور").map(String::as_str), Some("fermver"));
        // Spellings that are ordinary Persian words have an empty
        // phonemes column, so they get no override.
        for persian in [
            "نود", "شیر", "منو", "دومین", "گو", "نت", "ارگ", "ورد", "سرور", "پست", "رم", "کابل",
            "گیت", "پرو", "پلی", "کش", "راست",
        ] {
            assert!(
                !overrides.contains_key(persian),
                "{persian} must have no phoneme override"
            );
        }
    }

    #[test]
    fn word_list_table_is_well_formed() {
        let mut keys: Vec<&str> = Vec::new();
        for (line_no, line) in include_str!("../assets/latin_fa.tsv").lines().enumerate() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let columns: Vec<&str> = line.split('\t').collect();
            assert_eq!(
                columns.len(),
                3,
                "line {} must have three columns: {line}",
                line_no + 1
            );
            let (written, persian, phonemes) = (columns[0], columns[1], columns[2]);
            assert!(
                !written.is_empty()
                    && written
                        .chars()
                        .all(|c| c.is_ascii_lowercase() && c.is_ascii_alphabetic()),
                "line {} key is not lower-case ASCII letters: {written}",
                line_no + 1
            );
            assert!(
                !persian.is_empty()
                    && persian
                        .chars()
                        .all(|c| is_arabic_script(c) || c == ' ' || c == ZWNJ),
                "line {} value has a char outside Persian letters, ZWNJ and spaces: {persian}",
                line_no + 1
            );
            const ALPHABET: &str = "aeoAiubptsjChxdzrSqfkglmnvy?;";
            // The phonemes column may be empty (no override); when it
            // is present it must use only the v2 alphabet.
            assert!(
                phonemes.chars().all(|c| c != ' ' && ALPHABET.contains(c)),
                "line {} phonemes use a char outside the v2 alphabet: {phonemes}",
                line_no + 1
            );
            keys.push(written);
        }
        // Two rows with the same Persian spelling have the same phonemes.
        let mut by_persian: std::collections::HashMap<&str, &str> =
            std::collections::HashMap::new();
        for line in include_str!("../assets/latin_fa.tsv").lines() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let columns: Vec<&str> = line.split('\t').collect();
            if let Some(previous) = by_persian.insert(columns[1], columns[2]) {
                assert_eq!(previous, columns[2], "duplicate Persian spelling");
            }
        }
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        assert_eq!(keys, sorted, "word list keys are not sorted");
        sorted.dedup();
        assert_eq!(keys.len(), sorted.len(), "word list has duplicate keys");
        assert!(
            (250..=400).contains(&keys.len()),
            "word list should have about 300 entries, has {}",
            keys.len()
        );
    }
}
