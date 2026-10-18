//! Persian text normalization for Pocket TTS (v1).
//!
//! Faithful Rust port of `normalize()` in the Farsi model's
//! `normalize_fa.py` (mehdi-hf/pocket-tts-farsi, itself copied from the
//! training repo's `training/farsi/normalize_fa.py`): fold Arabic letter
//! forms onto Persian ones, drop diacritics and invisible controls (keeping
//! the ZWNJ), turn digits into Persian words, fold punctuation onto the kept
//! set, and drop anything else. The one omission is Unicode NFC composition
//! (std has none): the model text is already NFC in practice, and every
//! combining mark NFC would compose around is either folded or dropped here.

/// The zero-width non-joiner: phonemically meaningful in Persian.
const ZWNJ: char = '\u{200c}';

/// The 32 Persian letters plus ئ, in the spelling the model expects.
const PERSIAN_LETTERS: &str = "آابپتثجچحخدذرزژسشصضطظعغفقکگلمنوهیئ";

/// Punctuation kept through normalization (prosody the tokenizer learned).
const KEPT_PUNCT: &str = ".،؛؟!:";

fn is_persian_letter(c: char) -> bool {
    PERSIAN_LETTERS.contains(c)
}

fn is_allowed(c: char) -> bool {
    is_persian_letter(c) || c == ZWNJ || c == ' ' || KEPT_PUNCT.contains(c)
}

fn is_diacritic(c: char) -> bool {
    matches!(c, '\u{064b}'..='\u{065f}' | '\u{0670}' | '\u{06d6}'..='\u{06ed}')
}

fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{200b}' | '\u{200d}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}' | '\u{00ad}'
    )
}

/// Arabic letter forms folded onto Persian ones (plus dropped marks).
fn fold_char(c: char) -> Option<char> {
    Some(match c {
        '\u{064a}' | '\u{0649}' | '\u{06cd}' | '\u{06d0}' => 'ی',
        '\u{0643}' | '\u{06aa}' => 'ک',
        '\u{0629}' | '\u{06c0}' | '\u{06c2}' | '\u{06c3}' => 'ه',
        '\u{0623}' | '\u{0625}' | '\u{0671}' | '\u{0672}' | '\u{0673}' => 'ا',
        '\u{0624}' => 'و',
        '\u{0621}' | '\u{0640}' => return None,
        other => other,
    })
}

fn fold_digit(c: char) -> char {
    match c {
        '\u{06f0}'..='\u{06f9}' => char::from_digit(c as u32 - 0x06f0, 10).unwrap_or(c),
        '\u{0660}'..='\u{0669}' => char::from_digit(c as u32 - 0x0660, 10).unwrap_or(c),
        other => other,
    }
}

fn fold_punct(c: char) -> Option<char> {
    Some(match c {
        ',' => '،',
        '?' => '؟',
        ';' => '؛',
        '\u{066c}' => return None,
        '\u{066b}' | '…' => '.',
        '»' | '«' | '“' | '”' | '‘' | '’' | '"' | '\'' | '`' | '(' | ')' | '[' | ']' | '{'
        | '}' | '–' | '—' | '−' | '-' | '‐' | '/' | '\\' | '*' | '_' | '|' | '×' | '=' | '+' => {
            ' '
        }
        other => other,
    })
}

const ONES: [&str; 10] = [
    "", "یک", "دو", "سه", "چهار", "پنج", "شش", "هفت", "هشت", "نه",
];
const TEENS: [&str; 10] = [
    "ده",
    "یازده",
    "دوازده",
    "سیزده",
    "چهارده",
    "پانزده",
    "شانزده",
    "هفده",
    "هجده",
    "نوزده",
];
const TENS: [&str; 10] = [
    "",
    "",
    "بیست",
    "سی",
    "چهل",
    "پنجاه",
    "شصت",
    "هفتاد",
    "هشتاد",
    "نود",
];
const HUNDREDS: [&str; 10] = [
    "",
    "صد",
    "دویست",
    "سیصد",
    "چهارصد",
    "پانصد",
    "ششصد",
    "هفتصد",
    "هشتصد",
    "نهصد",
];
const SCALES: [(u64, &str); 4] = [
    (1_000_000_000_000, "تریلیون"),
    (1_000_000_000, "میلیارد"),
    (1_000_000, "میلیون"),
    (1_000, "هزار"),
];
const FRACTION_UNITS: [&str; 4] = ["", "دهم", "صدم", "هزارم"];
const JOIN: &str = " و ";
/// Above this many digits a run is read one digit at a time.
const MAX_NUMBER_DIGITS: usize = 12;

fn under_thousand(mut n: u64) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if n >= 100 {
        parts.push(HUNDREDS[(n / 100) as usize]);
        n %= 100;
    }
    if n >= 20 {
        parts.push(TENS[(n / 10) as usize]);
        n %= 10;
    }
    if (10..20).contains(&n) {
        parts.push(TEENS[(n - 10) as usize]);
        n = 0;
    }
    if n > 0 {
        parts.push(ONES[n as usize]);
    }
    parts.join(JOIN)
}

/// A non-negative integer as Persian words (`1402` → `هزار و چهارصد و دو`).
fn number_to_words(mut n: u64) -> String {
    if n == 0 {
        return "صفر".to_string();
    }
    let mut parts: Vec<String> = Vec::new();
    for (value, name) in SCALES {
        if n >= value {
            let count = n / value;
            n %= value;
            // "هزار" rather than "یک هزار"; "یک میلیون" is the normal form.
            let head = if count == 1 && value == 1_000 {
                String::new()
            } else {
                format!("{} ", under_thousand(count))
            };
            parts.push(format!("{head}{name}"));
        }
    }
    if n > 0 {
        parts.push(under_thousand(n));
    }
    parts.join(JOIN)
}

fn digits_one_by_one(digits: &str) -> String {
    digits
        .chars()
        .map(|d| {
            if d == '0' {
                "صفر"
            } else {
                ONES[d.to_digit(10).unwrap_or(0) as usize]
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn number_token_to_words(whole: &str, frac: Option<&str>) -> String {
    if whole.len() > MAX_NUMBER_DIGITS || (whole.starts_with('0') && whole.len() > 1) {
        let mut out = format!(" {}", digits_one_by_one(whole));
        if let Some(frac) = frac {
            out.push(' ');
            out.push_str(&digits_one_by_one(frac));
        }
        out.push(' ');
        return out;
    }
    let trimmed = whole.trim_start_matches('0');
    let value: u64 = if trimmed.is_empty() {
        0
    } else {
        trimmed.parse().unwrap_or(0)
    };
    let mut words = number_to_words(value);
    if let Some(frac) = frac {
        let frac = frac.trim_end_matches('0');
        if !frac.is_empty() {
            if frac.len() <= 3 {
                let frac_value: u64 = frac.parse().unwrap_or(0);
                words.push_str(&format!(
                    " ممیز {} {}",
                    number_to_words(frac_value),
                    FRACTION_UNITS[frac.len()]
                ));
            } else {
                words.push_str(&format!(" ممیز {}", digits_one_by_one(frac)));
            }
        }
    }
    format!(" {words} ")
}

fn numbers_to_words(text: &str) -> String {
    // Thousands separators go first: "1,234,567" -> "1234567". A comma is a
    // separator only between digits with exactly three digits after it.
    let chars: Vec<char> = text.chars().collect();
    let mut cleaned = String::with_capacity(text.len());
    for (i, c) in chars.iter().enumerate() {
        if *c == ','
            && i > 0
            && chars[i - 1].is_ascii_digit()
            && i + 3 < chars.len()
            && chars[i + 1].is_ascii_digit()
            && chars[i + 2].is_ascii_digit()
            && chars[i + 3].is_ascii_digit()
            && (i + 4 >= chars.len() || !chars[i + 4].is_ascii_digit())
        {
            continue;
        }
        cleaned.push(*c);
    }
    // Replace each (\d+)(?:\.(\d+))? run with its reading.
    let chars: Vec<char> = cleaned.chars().collect();
    let mut out = String::with_capacity(cleaned.len());
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_ascii_digit() {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && chars[i].is_ascii_digit() {
            i += 1;
        }
        let whole: String = chars[start..i].iter().collect();
        let mut frac: Option<String> = None;
        if i + 1 < chars.len() && chars[i] == '.' && chars[i + 1].is_ascii_digit() {
            i += 1;
            let frac_start = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            frac = Some(chars[frac_start..i].iter().collect());
        }
        out.push_str(&number_token_to_words(&whole, frac.as_deref()));
    }
    out
}

/// Fold `text` onto the canonical Persian spelling used for training, exactly
/// as the model's `normalize()` does. The result holds only Persian letters,
/// ZWNJ, spaces, and the kept punctuation.
#[must_use]
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if is_invisible(c) || is_diacritic(c) {
            continue;
        }
        let Some(c) = fold_char(c) else { continue };
        out.push(fold_digit(c));
    }
    // Number-internal separators go before the digit runs are read:
    // "۱٬۲۵۰٬۰۰۰" is one number, "۳٫۵" is one decimal.
    let out = out.replace('\u{066c}', "").replace('\u{066b}', ".");
    let out = out.replace(['٪', '%'], " درصد ");
    let out = numbers_to_words(&out);
    let mut folded = String::with_capacity(out.len());
    for c in out.chars() {
        if let Some(c) = fold_punct(c) {
            folded.push(c);
        }
    }
    // Anything still outside the allowed set becomes a space rather than
    // being glued onto its neighbours.
    let mut filtered = String::with_capacity(folded.len());
    for c in folded.chars() {
        filtered.push(if is_allowed(c) { c } else { ' ' });
    }
    // ZWNJ only means something between two letters.
    let mut collapsed = String::new();
    let mut last_zwnj = false;
    for c in filtered.chars() {
        if c == ZWNJ {
            if last_zwnj {
                continue;
            }
            last_zwnj = true;
        } else {
            last_zwnj = false;
        }
        collapsed.push(c);
    }
    // `\s*ZWNJ\s*` -> ZWNJ when no space touches it, else a single space.
    let chars: Vec<char> = collapsed.chars().collect();
    let mut spaced = String::with_capacity(collapsed.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_whitespace() || chars[i] == ZWNJ {
            let mut j = i;
            let mut has_space = false;
            let mut has_zwnj = false;
            while j < chars.len() && (chars[j].is_whitespace() || chars[j] == ZWNJ) {
                has_space |= chars[j] != ZWNJ;
                has_zwnj |= chars[j] == ZWNJ;
                j += 1;
            }
            if has_zwnj && !has_space {
                spaced.push(ZWNJ);
            } else if has_zwnj || has_space {
                spaced.push(' ');
            }
            i = j;
        } else {
            spaced.push(chars[i]);
            i += 1;
        }
    }
    // Drop a ZWNJ that no longer sits between two Persian letters.
    let chars: Vec<char> = spaced.chars().collect();
    let mut dezwnj = String::with_capacity(spaced.len());
    for (idx, c) in chars.iter().enumerate() {
        if *c == ZWNJ {
            let prev_ok = idx > 0 && is_persian_letter(chars[idx - 1]);
            let next_ok = idx + 1 < chars.len() && is_persian_letter(chars[idx + 1]);
            if !prev_ok || !next_ok {
                continue;
            }
        }
        dezwnj.push(*c);
    }
    // No space before punctuation, no doubled punctuation, single spaces.
    let mut result = String::with_capacity(dezwnj.len());
    let mut last_punct: Option<char> = None;
    for c in dezwnj.chars() {
        if KEPT_PUNCT.contains(c) {
            while result.ends_with(' ') {
                result.pop();
            }
            if last_punct == Some(c) {
                continue;
            }
            last_punct = Some(c);
            result.push(c);
        } else if c == ' ' {
            if result.ends_with(' ') || result.is_empty() {
                continue;
            }
            result.push(' ');
        } else {
            last_punct = None;
            result.push(c);
        }
    }
    let result = result.trim().to_string();
    result
        .trim_start_matches(|c| KEPT_PUNCT.contains(c) || c == ' ')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_arabic_forms_and_diacritics() {
        // Arabic yeh/kaf + harakat fold onto Persian spellings.
        assert_eq!(normalize("يك كتابٌ"), "یک کتاب");
        assert_eq!(normalize("مدرسة"), "مدرسه");
    }

    #[test]
    fn digits_become_persian_words() {
        assert_eq!(normalize("۱۴۰۲"), "هزار و چهارصد و دو");
        assert_eq!(normalize("25"), "بیست و پنج");
        assert_eq!(
            normalize("قیمت ۱٬۲۵۰ تومان"),
            "قیمت هزار و دویست و پنجاه تومان"
        );
        assert_eq!(normalize("۳٫۵"), "سه ممیز پنج دهم");
        assert_eq!(normalize("۰۹۱۲"), "صفر نه یک دو");
        assert_eq!(normalize("۵۰٪"), "پنجاه درصد");
    }

    #[test]
    fn punctuation_folds_and_latin_drops_to_spaces() {
        assert_eq!(normalize("سلام, حال شما?"), "سلام، حال شما؟");
        assert_eq!(normalize("سلام hello دنیا"), "سلام دنیا");
        assert_eq!(normalize("...سلام!"), "سلام!");
    }

    #[test]
    fn keeps_zwnj_between_letters_only() {
        assert_eq!(normalize("می‌رود"), "می‌رود");
        assert_eq!(normalize(" می‌رود "), "می‌رود");
        assert_eq!(normalize("می‌ رود"), "می رود");
    }

    #[test]
    fn plain_persian_passes_through() {
        let text = "امروز هوای تهران آفتابی است.";
        assert_eq!(normalize(text), text);
    }
}
