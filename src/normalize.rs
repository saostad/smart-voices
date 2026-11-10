//! English text front end: written text → the words a person would say.
//!
//! Numbers, dates, times, money, codes, units, symbols and abbreviations are
//! rewritten right before synthesis (chunks keep their display text), so every
//! English voice reads "Order #4471 ships 14/03" the way a person would.
//!
//! Rust port of `underdog_woof/normalize.py` and `respell.py` from Underdog
//! Woof (<https://huggingface.co/ConwayResearch/Underdog-Woof>), Underdog
//! Non-Commercial License 1.0, Copyright 2026 Conway. The rules stay in the
//! original order, because each pass depends on what earlier passes left
//! behind: `tests/normalize_golden.rs` checks this port against the Python's
//! output. Reordering a rule here means regenerating that fixture on purpose.

use std::collections::HashMap;
use std::sync::LazyLock;

use fancy_regex::{Captures, Regex};
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

/// A regex compiled once, on first use. Patterns are fixed, so a bad one is a
/// programming error that the tests catch: every site runs on every input.
macro_rules! re {
    ($pattern:expr) => {{
        static RE: LazyLock<Regex> =
            LazyLock::new(|| Regex::new($pattern).expect("valid normalizer regex"));
        &*RE
    }};
}

/// [`re!`] for a pattern assembled from the word lists.
macro_rules! re_dyn {
    ($pattern:expr) => {{
        static RE: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(&$pattern).expect("valid normalizer regex"));
        &*RE
    }};
}

/// How to read a date whose day and month could be swapped, like 09/10/2026.
/// A date elsewhere in the same text that settles it (14/03) takes priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DateOrder {
    #[default]
    MonthFirst,
    DayFirst,
}

/// Which normalization passes run (see the `normalize_text`, `respell_names`
/// and `date_order` settings).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NormalizeOptions {
    /// Rewrite numbers, dates, money, codes, units and symbols into words.
    pub normalize_text: bool,
    /// Respell hard names and foreign words from the built-in lexicon.
    pub respell_names: bool,
    /// How ambiguous slashed dates are read.
    pub date_order: DateOrder,
}

impl Default for NormalizeOptions {
    fn default() -> Self {
        Self {
            normalize_text: true,
            // Off by default: it can hurt names a voice already says well.
            respell_names: false,
            date_order: DateOrder::MonthFirst,
        }
    }
}

/// The text an English voice should read for `text`.
#[must_use]
pub fn normalize(text: &str, options: NormalizeOptions) -> String {
    if !options.normalize_text && !options.respell_names {
        return text.to_string();
    }
    let mut t = text.to_string();
    if options.normalize_text {
        let order = resolve_date_order(&t, options.date_order);
        t = norm_pre(&t, order);
        if search(re!(r"[0-9#%$€£@&/_+=<>±≥≤×°]"), &t) {
            t = fallback(&t, order);
        }
        t = tpl(re!(r"\s+([.,!?;:])"), &t, r"\1");
        t = tpl(re!(r"\s{2,}"), &t, " ").trim().to_string();
    }
    if options.respell_names {
        t = respell(&t);
    }
    if text.chars().next().is_some_and(char::is_uppercase) {
        capitalize_first(&t)
    } else {
        t
    }
}

/// One regex match plus the string it was found in.
struct M<'c, 't> {
    caps: &'c Captures<'t>,
    text: &'t str,
}

impl<'t> M<'_, 't> {
    /// Group `index`, or `""` when it didn't take part (Python's falsy `None`).
    fn g(&self, index: usize) -> &'t str {
        self.caps.get(index).map_or("", |m| m.as_str())
    }
    fn whole(&self) -> String {
        self.g(0).to_string()
    }
    fn before(&self) -> &'t str {
        &self.text[..self.caps.get(0).map_or(0, |m| m.start())]
    }
    fn after(&self) -> &'t str {
        &self.text[self.caps.get(0).map_or(self.text.len(), |m| m.end())..]
    }
}

/// A rule that hit the regex engine's backtracking limit is skipped for this
/// text. The voice then reads what the rule would have rewritten as written,
/// so it is a fallback: logged, and the reading continues.
fn warn_rule_skipped(error: &fancy_regex::Error) {
    eprintln!(
        "warn: fallback: true, attempted=text_normalization_rule, reason={error}, taken=rule_skipped"
    );
}

/// `re.sub` with a function.
fn sub(re: &Regex, text: &str, mut replace: impl FnMut(&M) -> String) -> String {
    let mut result = String::with_capacity(text.len());
    let mut last = 0;
    for caps in re.captures_iter(text) {
        let caps = match caps {
            Ok(caps) => caps,
            Err(error) => {
                warn_rule_skipped(&error);
                return text.to_string();
            }
        };
        let Some(whole) = caps.get(0) else { continue };
        result.push_str(&text[last..whole.start()]);
        result.push_str(&replace(&M { caps: &caps, text }));
        last = whole.end();
    }
    result.push_str(&text[last..]);
    result
}

/// `re.sub` with a template: `\1`-style group references, the rest literal.
fn tpl(re: &Regex, text: &str, template: &str) -> String {
    sub(re, text, |m| {
        let mut out = String::with_capacity(template.len());
        let mut chars = template.chars().peekable();
        while let Some(c) = chars.next() {
            match (c, chars.peek().and_then(|d| d.to_digit(10))) {
                ('\\', Some(group)) => {
                    chars.next();
                    out.push_str(m.g(group as usize));
                }
                _ => out.push(c),
            }
        }
        out
    })
}

/// `re.search`: true when `re` matches anywhere in `text`.
fn search(re: &Regex, text: &str) -> bool {
    re.is_match(text).unwrap_or_else(|error| {
        warn_rule_skipped(&error);
        false
    })
}

/// Python's `s[-n:]` in chars.
fn last_chars(s: &str, n: usize) -> &str {
    match s.char_indices().rev().nth(n.saturating_sub(1)) {
        Some((index, _)) if n > 0 => &s[index..],
        _ if n == 0 => "",
        _ => s,
    }
}

/// Python's `s[:n]` in chars.
fn first_chars(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((index, _)) => &s[..index],
        None => s,
    }
}

/// The order every slashed date in `text` should use, so one text never reads
/// 09/10/2026 month first and 12/10 day first. A date that only works one way
/// (14/03, 12-25-1999) settles it; otherwise the setting decides.
fn resolve_date_order(text: &str, preferred: DateOrder) -> DateOrder {
    let dates = re!(
        r"(?<![\d/.,])\b(\d{1,2})/(\d{1,2})(?:/\d{2,4})?\b(?!/\d)|\b(\d{1,2})-(\d{1,2})-(?:19|20)\d\d\b"
    );
    for caps in dates.captures_iter(text).flatten() {
        let group = |a: usize, b: usize| {
            caps.get(a)
                .or_else(|| caps.get(b))
                .map_or("", |g| g.as_str())
        };
        let (first, second) = (int(group(1, 3)), int(group(2, 4)));
        if (13..=31).contains(&first) && (1..=12).contains(&second) {
            return DateOrder::DayFirst;
        }
        if (13..=31).contains(&second) && (1..=12).contains(&first) {
            return DateOrder::MonthFirst;
        }
    }
    preferred
}

// MARK: - String helpers

fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Python's `str.capitalize`.
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first
            .to_uppercase()
            .chain(chars.flat_map(char::to_lowercase))
            .collect(),
        None => String::new(),
    }
}

fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_numeric())
}

fn is_alpha(s: &str) -> bool {
    !s.is_empty() && s.chars().all(char::is_alphabetic)
}

/// Python's `str.isupper`: has a cased char and every cased char is upper.
fn is_upper(s: &str) -> bool {
    s.chars().any(char::is_uppercase) && !s.chars().any(char::is_lowercase)
}

fn is_lower(s: &str) -> bool {
    s.chars().any(char::is_lowercase) && !s.chars().any(char::is_uppercase)
}

/// Parse digits, saturating instead of failing, so no input can panic here.
fn int(s: &str) -> u128 {
    s.chars()
        .filter_map(|c| c.to_digit(10))
        .fold(0u128, |n, d| {
            n.saturating_mul(10).saturating_add(u128::from(d))
        })
}

fn count(s: &str) -> usize {
    s.chars().count()
}

// MARK: - Numbers as words

const DIG: [&str; 10] = [
    "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
];
const ONES: [&str; 20] = [
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
];
const TENS: [&str; 10] = [
    "_", "_", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
];
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

fn cardinal(n: u128) -> String {
    if n < 20 {
        return ONES[n as usize].to_string();
    }
    if n < 100 {
        let tens = TENS[(n / 10) as usize];
        return if n.is_multiple_of(10) {
            tens.to_string()
        } else {
            format!("{tens} {}", ONES[(n % 10) as usize])
        };
    }
    if n < 1000 {
        let hundreds = ONES[(n / 100) as usize];
        return if n.is_multiple_of(100) {
            format!("{hundreds} hundred")
        } else {
            format!("{hundreds} hundred and {}", cardinal(n % 100))
        };
    }
    for (value, name) in [
        (1_000_000_000_000u128, "trillion"),
        (1_000_000_000, "billion"),
        (1_000_000, "million"),
        (1000, "thousand"),
    ] {
        if n >= value {
            let rest = n % value;
            let tail = if rest == 0 {
                String::new()
            } else if rest < 100 {
                format!(" and {}", cardinal(rest))
            } else {
                format!(" {}", cardinal(rest))
            };
            return format!("{} {name}{tail}", cardinal(n / value));
        }
    }
    n.to_string()
}

fn ordinal(n: u128) -> String {
    let words = cardinal(n);
    let (head, last) = match words.rsplit_once(' ') {
        Some((head, last)) => (format!("{head} "), last.to_string()),
        None => (String::new(), words),
    };
    let last = match last.as_str() {
        "one" => "first".to_string(),
        "two" => "second".to_string(),
        "three" => "third".to_string(),
        "five" => "fifth".to_string(),
        "eight" => "eighth".to_string(),
        "nine" => "ninth".to_string(),
        "twelve" => "twelfth".to_string(),
        other => match other.strip_suffix('y') {
            Some(stem) => format!("{stem}ieth"),
            None => format!("{other}th"),
        },
    };
    head + &last
}

fn hundred_pairs(n: u128) -> String {
    let (hi, lo) = (n / 100, n % 100);
    cardinal(hi)
        + &if lo == 0 {
            " hundred".to_string()
        } else if lo < 10 {
            format!(" oh {}", ONES[lo as usize])
        } else {
            format!(" {}", cardinal(lo))
        }
}

fn year(n: u128) -> String {
    if (2000..=2009).contains(&n) {
        cardinal(n)
    } else {
        hundred_pairs(n)
    }
}

fn number(s: &str) -> String {
    let s = s.replace(',', "");
    match s.split_once('.') {
        Some((a, b)) => {
            let whole = if a.is_empty() {
                "zero".to_string()
            } else {
                cardinal(int(a))
            };
            let decimals: Vec<&str> = b
                .chars()
                .map(|c| ONES[c.to_digit(10).unwrap_or(0) as usize])
                .collect();
            format!("{whole} point {}", decimals.join(" "))
        }
        None => cardinal(int(&s)),
    }
}

fn digits(s: &str) -> String {
    s.chars()
        .filter(char::is_ascii_digit)
        .map(|c| DIG[c.to_digit(10).unwrap_or(0) as usize])
        .collect::<Vec<_>>()
        .join(" ")
}

fn spell(s: &str) -> String {
    let mut out = Vec::new();
    for c in s.chars() {
        if c.is_ascii_digit() {
            out.push(DIG[c.to_digit(10).unwrap_or(0) as usize].to_string());
        } else if c.is_alphabetic() {
            out.push(c.to_uppercase().collect());
        }
    }
    out.join(" ")
}

/// All-caps chunk that reads as a word (SAVE, WINTER, PROMO) rather than
/// letters (ORD, XT, CS).
fn pronounceable(w: &str) -> bool {
    let w = w.to_lowercase();
    if count(&w) < 4 || !search(re!(r"[aeiouy]"), &w) {
        return false;
    }
    if search(re!(r"[^aeiouy]{3,}"), &w)
        && !search(re!(r"(str|sch|thr|chr|spr|scr|ght|nds|nts)"), &w)
    {
        return false;
    }
    search(re!(r"^(?:[^aeiouy]{0,3}[aeiouy]{1,2})+[^aeiouy]{0,3}$"), &w)
}

fn code_read(s: &str) -> String {
    let mut out = Vec::new();
    for p in s.split(['-', '/']) {
        if p.is_empty() {
            continue;
        }
        if is_digits(p) {
            out.push(if count(p) <= 2 && !p.starts_with('0') {
                p.to_string()
            } else {
                digits(p)
            });
        } else if is_alpha(p) && is_lower(p) && count(p) > 3 && pronounceable(p) {
            out.push(p.to_string());
        } else if is_alpha(p) && pronounceable(p) {
            out.push(capitalize(p));
        } else {
            out.push(spell(p));
        }
    }
    out.join(" ")
}

/// Digits of an identifier read the way people say room, flight and house
/// numbers: 402 → four oh two, 229 → two twenty nine, 1204 → twelve oh four,
/// 1600 → sixteen hundred; 5+ digits or a leading 0 go digit by digit.
fn nread(s: &str) -> String {
    let n = int(s);
    let len = count(s);
    if s.starts_with('0') && len > 1 || len > 4 {
        return digits(s);
    }
    if n < 100 || n.is_multiple_of(100) && len == 3 || n.is_multiple_of(1000) {
        return cardinal(n);
    }
    hundred_pairs(n)
}

/// getUserName → get user name, `user_id` → user id, OpenAI → Open A I.
fn split_ident(s: &str) -> String {
    let spaced = s.replace('_', " ");
    let spaced = tpl(
        re!(r"(?<=[a-z0-9])(?=[A-Z])|(?<=[A-Z])(?=[A-Z][a-z])"),
        &spaced,
        " ",
    );
    spaced
        .split_whitespace()
        .map(|w| {
            if is_upper(w) && count(w) <= 4 {
                spell(w)
            } else {
                w.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// @openai → open A I, `@Jane_Doe` → Jane underscore Doe.
fn handle(h: &str) -> String {
    let parts: Vec<String> = h
        .split('_')
        .map(|p| match re!(r"^([a-z]{3,}[nlyex])(ai|hq)$").captures(p) {
            Ok(Some(caps)) => {
                format!(
                    "{} {}",
                    caps.get(1).map_or("", |m| m.as_str()),
                    spell(caps.get(2).map_or("", |m| m.as_str()))
                )
            }
            _ => split_ident(p),
        })
        .filter(|x| !x.is_empty())
        .collect();
    parts.join(" underscore ")
}

// MARK: - Roman numerals

const ROMAN_RE: &str =
    r"(?=[MDCLXVI]+\b)M{0,3}(?:CM|CD|D?C{0,3})(?:XC|XL|L?X{0,3})(?:IX|IV|V?I{0,3})";
/// Words after which a Roman numeral is a plain count (Chapter XIV, World War II).
const ROMAN_CARD_WORDS: &[&str] = &[
    "chapter",
    "chap",
    "ch",
    "part",
    "pt",
    "book",
    "volume",
    "vol",
    "act",
    "scene",
    "section",
    "sec",
    "phase",
    "stage",
    "level",
    "episode",
    "season",
    "article",
    "title",
    "class",
    "type",
    "grade",
    "war",
    "bowl",
    "apollo",
    "mark",
    "mk",
    "vatican",
    "rocky",
    "fantasy",
    "wars",
    "trek",
    "civilization",
    "gemini",
    "saturn",
    "soyuz",
    "annex",
    "appendix",
    "figure",
    "fig",
    "table",
    "plate",
    "tier",
    "round",
    "schedule",
    "unit",
    "division",
    "category",
    "series",
    "olympiad",
    "psalm",
    "canto",
    "lesson",
    "module",
    "form",
];
const ROMAN_LABEL: &[&str] = &[
    "appendix", "annex", "table", "figure", "fig", "plate", "schedule", "unit", "category",
    "class", "type", "grade", "tier", "section", "form", "division", "series",
];
const ROMAN_STOP: &[&str] = &[
    "the", "a", "an", "in", "on", "of", "and", "for", "to", "at", "by", "from", "with", "my",
    "our", "his", "her", "their", "this", "that", "its",
];
/// Names that take a regnal ordinal (Henry VIII → Henry the eighth).
const REGNAL: &[&str] = &[
    "Henry",
    "Louis",
    "Elizabeth",
    "George",
    "Edward",
    "Richard",
    "Charles",
    "James",
    "William",
    "John",
    "Paul",
    "Benedict",
    "Pius",
    "Leo",
    "Gregory",
    "Innocent",
    "Clement",
    "Boniface",
    "Urban",
    "Alexander",
    "Peter",
    "Ivan",
    "Nicholas",
    "Frederick",
    "Wilhelm",
    "Ludwig",
    "Philip",
    "Felipe",
    "Carlos",
    "Gustav",
    "Gustaf",
    "Christian",
    "Olaf",
    "Haakon",
    "Harald",
    "Ramesses",
    "Ramses",
    "Thutmose",
    "Amenhotep",
    "Ptolemy",
    "Darius",
    "Xerxes",
    "Cyrus",
    "Constantine",
    "Justinian",
    "Mehmed",
    "Suleiman",
    "Selim",
    "Murad",
    "Napoleon",
    "Umberto",
    "Mary",
    "Anne",
    "Victoria",
    "Catherine",
    "Isabella",
    "Margaret",
    "Rama",
    "Abdullah",
    "Robert",
    "David",
    "Kenneth",
    "Duncan",
    "Alfonso",
    "Ferdinand",
    "Sancho",
    "Manuel",
    "Sebastian",
    "Stephen",
    "Matthias",
    "Sigismund",
    "Casimir",
    "Otto",
    "Conrad",
    "Rudolf",
    "Maximilian",
    "Leopold",
    "Joseph",
    "Francis",
    "Franz",
    "Karl",
    "Hugh",
    "Baldwin",
    "Basil",
    "Michael",
    "Theodore",
    "Edmund",
    "Harold",
    "Magnus",
    "Eric",
    "Erik",
    "Valdemar",
    "Frederik",
    "Albert",
    "Baudouin",
    "Philippe",
    "Rainier",
    "Hassan",
    "Faisal",
    "Fuad",
    "Pedro",
    "Afonso",
    "Jaime",
    "Sixtus",
    "Julius",
    "Adrian",
    "Hadrian",
    "Celestine",
    "Honorius",
    "Lothair",
    "Pepin",
    "Antiochus",
    "Seleucus",
    "Herod",
    "Cleopatra",
    "Wenceslaus",
    "Kamehameha",
    "Tewodros",
    "Menelik",
    "Nebuchadnezzar",
    "Shalmaneser",
    "Sargon",
    "Tiglath",
    "Artaxerxes",
    "Andronikos",
    "Romanos",
    "Leon",
    "Mohammed",
    "Muhammad",
    "Charlemagne",
];
const TITLES: &[&str] = &[
    "King", "Queen", "Pope", "Emperor", "Empress", "Tsar", "Czar", "Tsarina", "Kaiser", "Pharaoh",
    "Sultan", "Shah", "Prince", "Princess", "Duke", "Duchess", "Count", "Countess", "Lord", "Lady",
    "Saint", "St", "Archduke", "Elector", "Emir", "Caliph", "Khan", "King's", "Queen's",
];

fn roman(s: &str) -> u128 {
    let value = |c: char| -> u128 {
        match c {
            'I' => 1,
            'V' => 5,
            'X' => 10,
            'L' => 50,
            'C' => 100,
            'D' => 500,
            'M' => 1000,
            _ => 0,
        }
    };
    let chars: Vec<char> = s.chars().collect();
    let mut total: i128 = 0;
    for (index, &c) in chars.iter().enumerate() {
        let v = value(c) as i128;
        if chars.get(index + 1).is_some_and(|&b| value(b) as i128 > v) {
            total -= v;
        } else {
            total += v;
        }
    }
    total.max(0) as u128
}

fn is_title_word(w: &str) -> bool {
    search(re!(r"^[A-Z][a-z]+$"), w)
}

fn is_roman_card(key: &str) -> bool {
    key == "through"
        || key == "thru"
        || ROMAN_CARD_WORDS.contains(&key)
        || key
            .strip_suffix('s')
            .is_some_and(|stem| ROMAN_CARD_WORDS.contains(&stem))
}

fn roman_sub(m: &M) -> String {
    let (prev, gap, num) = (m.g(1), m.g(2), m.g(3));
    if num.is_empty() {
        return m.whole();
    }
    let last_before = m.before().split_whitespace().last();
    let prev2 = match last_before {
        Some(word) if !search(re!(r#"[.,;:!?)"]$"#), word) => word,
        _ => "",
    };
    let lowered = prev.to_lowercase();
    let key = lowered.trim_end_matches('.');
    let n = roman(num);
    if matches!(key, "through" | "thru" | "to" | "and" | "or")
        && !search(
            re_dyn!({
                let mut words: Vec<String> =
                    ROMAN_CARD_WORDS.iter().map(ToString::to_string).collect();
                words.extend(ROMAN_CARD_WORDS.iter().map(|w| format!("{w}s")));
                words.extend(["through".to_string(), "thru".to_string()]);
                format!(r"(?i)\b(?:{})\s+\S+\s+$", words.join("|"))
            }),
            m.before(),
        )
    {
        return m.whole();
    }
    if is_roman_card(key)
        && (count(num) > 1
            || num == "I" && prev.chars().next().is_some_and(char::is_uppercase)
            || matches!(num, "V" | "X") && !ROMAN_LABEL.contains(&key))
    {
        return format!("{prev}{gap}{}", cardinal(n));
    }
    if !search(re!(r"^[IVXL]+$"), num) {
        return m.whole();
    }
    if REGNAL.contains(&prev) || TITLES.contains(&prev2) && is_title_word(prev) && num != "I" {
        return format!("{prev}{gap}the {}", ordinal(n));
    }
    if count(num) > 1 && is_title_word(prev) && !search(re!(r"^[IVXL]*L[IVXL]*$"), num) {
        // John Smith III → the third; Frozen II, Rocky IV → two, four.
        let full = is_title_word(prev2) && !ROMAN_STOP.contains(&prev2.to_lowercase().as_str());
        return if full {
            format!("{prev}{gap}the {}", ordinal(n))
        } else {
            format!("{prev}{gap}{}", cardinal(n))
        };
    }
    m.whole()
}

// MARK: - Word lists

const CURCODE: &[(&str, &str)] = &[
    ("USD", "$"),
    ("US$", "$"),
    ("EUR", "€"),
    ("GBP", "£"),
    ("JPY", "yen"),
    ("¥", "yen"),
    ("CNY", "yuan"),
    ("RMB", "yuan"),
    ("INR", "rupees"),
    ("₹", "rupees"),
    ("Rs.", "rupees"),
    ("Rs", "rupees"),
    ("CAD", "Canadian dollars"),
    ("C$", "Canadian dollars"),
    ("AUD", "Australian dollars"),
    ("A$", "Australian dollars"),
    ("NZD", "New Zealand dollars"),
    ("CHF", "Swiss francs"),
    ("HKD", "Hong Kong dollars"),
    ("HK$", "Hong Kong dollars"),
    ("SGD", "Singapore dollars"),
    ("KRW", "won"),
    ("₩", "won"),
    ("MXN", "pesos"),
    ("BRL", "reais"),
    ("R$", "reais"),
    ("RUB", "rubles"),
    ("₽", "rubles"),
    ("SEK", "Swedish kronor"),
    ("NOK", "Norwegian kroner"),
    ("DKK", "Danish kroner"),
    ("ZAR", "rand"),
    ("₦", "naira"),
    ("NGN", "naira"),
    ("₺", "lira"),
    ("₱", "pesos"),
    ("₫", "dong"),
    ("BTC", "bitcoin"),
    ("ETH", "ether"),
    ("₿", "bitcoin"),
];

fn lookup<'a>(table: &'a [(&'a str, &'a str)], key: &str) -> Option<&'a str> {
    table.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

fn scale(key: &str) -> &'static str {
    match key {
        "k" | "K" | "thousand" => "thousand",
        "m" | "M" | "mn" | "mm" | "million" => "million",
        "bn" | "b" | "B" | "billion" => "billion",
        "tn" | "T" | "trn" | "trillion" => "trillion",
        _ => "",
    }
}

const FRAC_DEN: &[u128] = &[2, 3, 4, 5, 6, 8, 10, 16, 32, 64, 100];
const FRAC_AFTER: &str = r#"^\s*(?:of\b|an?\b|cups?|c\.|teaspoons?|tsp|tablespoons?|tbsp|inch|inches|in\b|in\.|"|miles?|mi\b|pounds?|lbs?|ounces?|oz|hours?|hrs?|minutes?|mins?|seconds?|pints?|quarts?|gallons?|liters?|litres?|kilos?|kg|feet|foot|ft|yards?|acres?|sticks?|pieces?|slices?|loaf|tons?|percent|points?|shares?|the\b|times\b|marathon|price|full|empty|done|finished|complete)"#;
const DATE_BEFORE: &str = r"(?i)(?:\b(?:on|by|until|till|from|since|before|after|due|dated?|deadline|through|thru|starting|effective|expires?|expiry|born|died|week of|shipped|delivered|arrives?|arriving|ordered|paid|placed|scheduled|filed|signed|posted|submitted|opens?|closes?|ends?|starts?|begins?|[Mm]onday|[Tt]uesday|[Ww]ednesday|[Tt]hursday|[Ff]riday|[Ss]aturday|[Ss]unday|Mon|Tue|Wed|Thu|Fri|Sat|Sun)\.?[,:]?\s*)$";

/// 3/4 → three quarters; 1/2 → one half; with `lead` (the 1 1/2 case): a half.
fn frac_words(a: u128, b: u128, lead: bool) -> String {
    let name = if b == 2 {
        if a == 1 { "half" } else { "halves" }.to_string()
    } else {
        let base = if b == 4 {
            "quarter".to_string()
        } else {
            ordinal(b)
        };
        if a == 1 { base } else { base + "s" }
    };
    let head = if lead && a == 1 {
        "a".to_string()
    } else {
        cardinal(a)
    };
    format!("{head} {name}")
}

const ABBR: &[(&str, &str)] = &[
    (r"\bDr\.(?=\s+[A-Z])", "Doctor"),
    (r"\bMr\.", "Mister"),
    (r"\bMrs\.", "Missus"),
    (r"\bMs\.", "Miz"),
    (r"\bProf\.(?=\s+[A-Z])", "Professor"),
    (r"\bSt\.(?=\s+[A-Z])", "Saint"),
    (r"(?<=[a-z] )St\.", "Street"),
    (r"\bRd\.", "Road"),
    (r"\bAve\.", "Avenue"),
    (r"\bBlvd\.", "Boulevard"),
    (r"\bApt\.", "Apartment"),
    (r"\b[Aa]pprox\.", "approximately"),
    (r"\betc\.", "et cetera"),
    (r"\be\.g\.", "for example"),
    (r"\bi\.e\.", "that is"),
    (r"\bvs\.?(?=\s)", "versus"),
    (r"\b[Nn]o\.(?=\s*\d)", "number"),
    (r"\bdept\.", "department"),
    (r"\bext\.", "extension"),
    (r"(?<=\d)\s?min\.", " minutes"),
    (r"\bmin\.(?=\s+[a-z])", "minimum"),
    (r"\bmin\.", "minutes"),
    (r"\bhr?s?\.(?=\s)", "hours"),
    (r"(?<![\d\s])\s*\b[Ss]ec\.(?=\s*\d)", " section"),
    (r"\b([Ee])st\.(?=\s*\d{4})", r"\1stablished"),
    (r"\bSgt\.", "Sergeant"),
    (r"\bCapt\.", "Captain"),
    (r"\bLt\.", "Lieutenant"),
    (r"\bCol\.", "Colonel"),
    (r"\bGen\.(?=\s+[A-Z])", "General"),
    (r"\bGov\.", "Governor"),
    (r"\bSen\.", "Senator"),
    (r"\bRep\.(?=\s+[A-Z])", "Representative"),
    (r"\bRev\.", "Reverend"),
    (r"\bHon\.", "Honorable"),
    (r"\bJr\.", "Junior"),
    (r"\bSr\.", "Senior"),
    (r"\bLn\.", "Lane"),
    (r"\bDr\.(?=[\s,.]|$)", "Drive"),
    (r"\bCt\.", "Court"),
    (r"\bPl\.", "Place"),
    (r"\bSq\.", "Square"),
    (r"\bHwy\.", "Highway"),
    (r"\bPkwy\.", "Parkway"),
    (r"\bMt\.", "Mount"),
    (r"\bFt\.(?=\s+[A-Z])", "Fort"),
    (r"\bSte\.", "Suite"),
    (r"\bBldg\.", "Building"),
    (r"\bfl\.", "floor"),
    (r"\bFl\.", "Floor"),
    (r"\bRm\.", "Room"),
    (r"\bEst\.", "Estimated"),
    (r"\best\.", "estimated"),
    (r"\bInc\.", "Incorporated"),
    (r"\bLtd\.", "Limited"),
    (r"\bCo\.(?=\s)", "Company"),
    (r"\bCorp\.", "Corporation"),
    (r"\bDept\.", "Department"),
    (r"\bAssn\.", "Association"),
    (r"\bUniv\.", "University"),
    (r"\bqty\.?", "quantity"),
    (r"\bref\.", "reference"),
    (r"\btel\.", "telephone"),
    (r"\bTel\.", "Telephone"),
    (r"\bfig\.", "figure"),
    (r"\bvol\.", "volume"),
    (r"\bpp\.", "pages"),
    (r"\bp\.(?=\s*\d)", "page"),
    (r"\bwk\.", "week"),
    (r"\bmo\.", "month"),
    (r"\byr\.", "year"),
    (r"\bavg\.", "average"),
    (r"\bmax\.", "maximum"),
];
const DAYS: &[(&str, &str)] = &[
    ("Mon", "Monday"),
    ("Tue", "Tuesday"),
    ("Tues", "Tuesday"),
    ("Wed", "Wednesday"),
    ("Thu", "Thursday"),
    ("Thur", "Thursday"),
    ("Thurs", "Thursday"),
    ("Fri", "Friday"),
    ("Sat", "Saturday"),
    ("Sun", "Sunday"),
];
const MON_ABBR: &[(&str, &str)] = &[
    ("Jan", "January"),
    ("Feb", "February"),
    ("Mar", "March"),
    ("Apr", "April"),
    ("Jun", "June"),
    ("Jul", "July"),
    ("Aug", "August"),
    ("Sep", "September"),
    ("Sept", "September"),
    ("Oct", "October"),
    ("Nov", "November"),
    ("Dec", "December"),
];
const SYMBOLS: &[(&str, &str)] = &[
    (r"\s*±\s*", " plus or minus "),
    (r"\s*≥\s*", " greater than or equal to "),
    (r"\s*≤\s*", " less than or equal to "),
    (r"\s*≠\s*", " not equal to "),
    (r"\s*≈\s*", " approximately "),
    (r"\s*÷\s*", " divided by "),
    (r"\s*→\s*", " to "),
    (r"\s*√\s*", " square root of "),
    (r"(?<=\d)\s*%", " percent"),
    (r"\s*‰", " per mille"),
    (r"\s+=\s+", " equals "),
    (r"(?<=\d)\s*<\s*(?=\d)", " less than "),
    (r"(?<=\d)\s*>\s*(?=\d)", " greater than "),
    (r"\s*&\s*", " and "),
];
const UNITS: &[(&str, &str)] = &[
    ("km", "kilometers"),
    ("m", "meters"),
    ("cm", "centimeters"),
    ("mm", "millimeters"),
    ("kg", "kilograms"),
    ("g", "grams"),
    ("mg", "milligrams"),
    ("l", "liters"),
    ("L", "liters"),
    ("ml", "milliliters"),
    ("mL", "milliliters"),
    ("km/h", "kilometers per hour"),
    ("mph", "miles per hour"),
    ("GB", "gigabytes"),
    ("MB", "megabytes"),
    ("TB", "terabytes"),
    ("kB", "kilobytes"),
    ("GHz", "gigahertz"),
    ("MHz", "megahertz"),
    ("kW", "kilowatts"),
    ("kWh", "kilowatt hours"),
    ("W", "watts"),
    ("V", "volts"),
    ("mAh", "milliamp hours"),
    ("ft", "feet"),
    ("in", "inches"),
    ("lb", "pounds"),
    ("lbs", "pounds"),
    ("oz", "ounces"),
    ("h", "hours"),
    ("min", "minutes"),
    ("s", "seconds"),
    ("ms", "milliseconds"),
    ("mi", "miles"),
    ("yd", "yards"),
    ("yds", "yards"),
    ("Hz", "hertz"),
    ("kHz", "kilohertz"),
    ("dB", "decibels"),
    ("mpg", "miles per gallon"),
    ("kph", "kilometers per hour"),
    ("tsp", "teaspoons"),
    ("tbsp", "tablespoons"),
    ("gal", "gallons"),
    ("qt", "quarts"),
    ("mcg", "micrograms"),
    ("µg", "micrograms"),
    ("μg", "micrograms"),
    ("nm", "nanometers"),
    ("µm", "micrometers"),
    ("μm", "micrometers"),
    ("ns", "nanoseconds"),
    ("µs", "microseconds"),
    ("μs", "microseconds"),
    ("sec", "seconds"),
    ("secs", "seconds"),
    ("Mbps", "megabits per second"),
    ("Gbps", "gigabits per second"),
    ("kbps", "kilobits per second"),
    ("kcal", "kilocalories"),
    ("mmHg", "millimeters of mercury"),
    ("dL", "deciliters"),
    ("dl", "deciliters"),
    ("PB", "petabytes"),
    ("MW", "megawatts"),
    ("GW", "gigawatts"),
    ("GWh", "gigawatt hours"),
    ("MWh", "megawatt hours"),
    ("kPa", "kilopascals"),
    ("hPa", "hectopascals"),
    ("mmol", "millimoles"),
    ("fl oz", "fluid ounces"),
    ("sq ft", "square feet"),
    ("sq m", "square meters"),
    ("km2", "square kilometers"),
    ("m2", "square meters"),
    ("cc", "C C"),
    ("mA", "milliamps"),
    ("ng", "nanograms"),
    ("pg", "picograms"),
    ("IU", "I U"),
    ("mEq", "milliequivalents"),
    ("kN", "kilonewtons"),
    ("Nm", "newton meters"),
    ("TFLOPS", "teraflops"),
    ("rpm", "R P M"),
    ("psi", "P S I"),
];

fn unit_name(key: &str) -> &'static str {
    lookup(UNITS, key).unwrap_or("")
}

fn sing(w: &str) -> String {
    let (head, sep, rest) = match w.split_once(" per ") {
        Some((head, rest)) => (head, " per ", rest),
        None => (w, "", ""),
    };
    if head.contains(" of ") {
        return w.to_string();
    }
    let mut words: Vec<&str> = head.split(' ').collect();
    let last = words.pop().unwrap_or("");
    let single = match last {
        "feet" => "foot",
        "inches" => "inch",
        "hertz" => "hertz",
        "kilohertz" => "kilohertz",
        "teaspoons" => "teaspoon",
        "C C" | "R P M" | "P S I" | "I U" => last,
        "teraflops" => "teraflop",
        other => other.strip_suffix('s').unwrap_or(other),
    };
    words.push(single);
    format!("{}{sep}{rest}", words.join(" "))
}

fn unit(key: &str, n: &str) -> String {
    if n == "1" {
        sing(unit_name(key))
    } else {
        unit_name(key).to_string()
    }
}

const MON_RE: &str =
    "(January|February|March|April|May|June|July|August|September|October|November|December)";
const STREET: &[(&str, &str)] = &[
    ("St", "Street"),
    ("Rd", "Road"),
    ("Ave", "Avenue"),
    ("Av", "Avenue"),
    ("Blvd", "Boulevard"),
    ("Ln", "Lane"),
    ("Dr", "Drive"),
    ("Ct", "Court"),
    ("Pl", "Place"),
    ("Sq", "Square"),
    ("Hwy", "Highway"),
    ("Pkwy", "Parkway"),
    ("Cres", "Crescent"),
    ("Ter", "Terrace"),
    ("Terr", "Terrace"),
    ("Cir", "Circle"),
    ("Trl", "Trail"),
    ("Pk", "Park"),
];
/// Words after which a 2-4 digit number is an identifier read in pairs.
const ID_WORDS: &str = r"[Rr]oom|[Rr]m\.?|[Oo]ffice|[Ss]uite|[Ss]te\.?|[Aa]partment|[Aa]pt\.?|[Uu]nit|[Gg]ate|[Pp]latform|[Ff]light|[Bb]us|[Rr]oute|[Hh]ighway|[Hh]wy\.?|[Ii]nterstate|[Tt]erminal|[Bb]ay|[Dd]ock|[Pp]ier|[Ss]eat|[Cc]abin|[Ww]ard|[Ll]ocker|[Bb]ooth|[Ss]tudio|[Bb]uilding|[Bb]ldg\.?|[Ee]xit|[Pp]ort|[Ee]xt\.?|[Ee]xtension|[Mm]odel|[Bb]oeing|[Aa]irbus|[Cc]heckpoint|[Hh]angar|[Tt]rain|[Cc]oach|[Ff]erry|[Cc]ar";
const ELEMENTS: &[&str] = &[
    "H", "He", "Li", "Be", "B", "C", "N", "O", "F", "Ne", "Na", "Mg", "Al", "Si", "P", "S", "Cl",
    "Ar", "K", "Ca", "Sc", "Ti", "V", "Cr", "Mn", "Fe", "Co", "Ni", "Cu", "Zn", "Ga", "Ge", "As",
    "Se", "Br", "Kr", "Rb", "Sr", "Y", "Zr", "Nb", "Mo", "Tc", "Ru", "Rh", "Pd", "Ag", "Cd", "In",
    "Sn", "Sb", "Te", "I", "Xe", "Cs", "Ba", "La", "Ce", "Pr", "Nd", "Pm", "Sm", "Eu", "Gd", "Tb",
    "Dy", "Ho", "Er", "Tm", "Yb", "Lu", "Hf", "Ta", "W", "Re", "Os", "Ir", "Pt", "Au", "Hg", "Tl",
    "Pb", "Bi", "Po", "At", "Rn", "Fr", "Ra", "Ac", "Th", "Pa", "U", "Np", "Pu", "Am", "Cm", "Bk",
    "Cf", "Es", "Fm", "Md", "No", "Lr",
];
const CODE_CTX: &str = r"(?i:\b(?:codes?|ref(?:erence)?|serial|PIN|password|passcode|quote|plate|registration|booking|confirmation|tracking|order|voucher|coupon|promo|ticket|ID|key|token|account|policy|claim|case|invoice|SKU|part)\b)[^.;!?]*$";
const UNIT_PRE: &str = r"(?<![\w.])(?<!\w-)(\d(?:[\d,]*\d)?(?:\.\d+)?)";
const DOMAIN_TLD: &str = "com|org|net|io|ai|co|uk|dev|app|gov|edu|ly|me|tv|us|eu|de|fr|info|biz";

fn scheme_say(s: &str) -> String {
    let lower = s.to_lowercase();
    match lower.as_str() {
        "http" => "H T T P".to_string(),
        "https" => "H T T P S".to_string(),
        "file" | "git" | "data" | "blob" | "chrome" | "spotify" | "slack" | "zoom" | "steam" => {
            lower
        }
        _ if pronounceable(s) => lower,
        _ => spell(s),
    }
}

fn keys(table: &[(&str, &str)]) -> String {
    table.iter().map(|(k, _)| *k).collect::<Vec<_>>().join("|")
}

/// Unit keys, longest first, without the two that read as ordinary words.
fn ukeys() -> String {
    let mut sorted: Vec<&str> = UNITS
        .iter()
        .map(|(k, _)| *k)
        .filter(|k| !matches!(*k, "in" | "s"))
        .collect();
    sorted.sort_by_key(|k| std::cmp::Reverse(count(k)));
    sorted
        .iter()
        .map(|k| regex::escape(k))
        .collect::<Vec<_>>()
        .join("|")
}

/// Currency codes and symbols, longest first.
fn currency_codes() -> String {
    let mut sorted: Vec<&str> = CURCODE.iter().map(|(k, _)| *k).collect();
    sorted.sort_by_key(|k| std::cmp::Reverse(count(k)));
    sorted
        .iter()
        .map(|k| regex::escape(k))
        .collect::<Vec<_>>()
        .join("|")
}

fn plural(n: &str, word: &str) -> String {
    format!("{n} {word}{}", if n == "1" { "" } else { "s" })
}

fn plural_int(n: &str, word: &str) -> String {
    let value = int(n);
    format!("{value} {word}{}", if value == 1 { "" } else { "s" })
}

fn hour_word(n: &str) -> String {
    plural(n, "hour")
}

// MARK: - First pass: tokens

const NUM: &str = r"(\d[\d,]*(?:\.\d+)?)";
const SC: &str = r"(\s?(?:thousand|million|billion|trillion)\b|(?:k|K|mn|m|M|bn|b|B|tn|T)\b)?";

fn currency(code: &str, num: &str, sc: &str) -> String {
    let Some(name) = lookup(CURCODE, code) else {
        return format!("{code}{num}{sc}");
    };
    if count(name) == 1 {
        return format!("{name}{num}{sc}");
    }
    let (whole, cents) = num.split_once('.').unwrap_or((num, ""));
    if sc.is_empty() && count(cents) == 2 && name.ends_with("dollars") {
        let tail = if int(cents) == 0 {
            String::new()
        } else {
            format!(" and {} cents", int(cents))
        };
        return format!("{whole} {name}{tail}");
    }
    let scaled = if sc.is_empty() {
        String::new()
    } else {
        format!(" {}", scale(sc.trim()))
    };
    format!("{num}{scaled} {name}")
}

fn money_word(symbol: &str) -> &'static str {
    match symbol {
        "$" => "dollars",
        "€" => "euros",
        _ => "pounds",
    }
}

fn decade(m: &M) -> String {
    let s = m.g(2);
    let n = int(s);
    if !n.is_multiple_of(10) {
        return m.whole();
    }
    let word = if count(s) == 4 {
        year(n)
    } else if n == 0 && !m.g(1).is_empty() {
        "two thousand".to_string()
    } else if n != 0 {
        cardinal(n)
    } else {
        String::new()
    };
    if word.is_empty() {
        return m.whole();
    }
    match word.strip_suffix('y') {
        Some(stem) => format!("{stem}ies"),
        None => format!("{word}s"),
    }
}

fn power(m: &M) -> String {
    let (base, exp) = (m.g(1), m.g(2));
    let neg = exp.starts_with('-');
    let exp = exp.trim_start_matches(['+', '-']);
    let base = if is_alpha(base) && count(base) > 1 {
        base.chars().map(String::from).collect::<Vec<_>>().join(" ")
    } else {
        base.to_string()
    };
    if !is_digits(exp) {
        return format!("{base} to the {exp}");
    }
    let n = int(exp);
    if n == 2 && !neg {
        format!("{base} squared")
    } else if n == 3 && !neg {
        format!("{base} cubed")
    } else {
        format!(
            "{base} to the {}{}",
            if neg { "minus " } else { "" },
            ordinal(n)
        )
    }
}

/// Rules that must see the raw string before the general passes: symbols
/// outside the voice's alphabet, money with scale words and ranges, decades,
/// Roman numerals, powers, durations, code identifiers, hashtags and handles.
#[allow(clippy::too_many_lines)]
fn norm_tokens(t: &str) -> String {
    let mut t = t
        .replace('−', "-")
        .replace(['’', '‘'], "'")
        .replace(['“', '”'], "\"");
    t = tpl(re!(r"(?<=\d)([½¼¾⅓⅔⅛])"), &t, r" \1");
    t = sub(re!(r"[½¼¾⅓⅔⅛]"), &t, |m| {
        match m.g(0) {
            "½" => "1/2",
            "¼" => "1/4",
            "¾" => "3/4",
            "⅓" => "1/3",
            "⅔" => "2/3",
            _ => "1/8",
        }
        .to_string()
    });
    t = sub(re!(r"[⁰¹²³⁴⁵⁶⁷⁸⁹⁻]+"), &t, |m| {
        let mapped: String = m
            .g(0)
            .chars()
            .map(|c| match c {
                '⁰' => '0',
                '¹' => '1',
                '²' => '2',
                '³' => '3',
                '⁴' => '4',
                '⁵' => '5',
                '⁶' => '6',
                '⁷' => '7',
                '⁸' => '8',
                '⁹' => '9',
                _ => '-',
            })
            .collect();
        format!("^{mapped}")
    });
    // Tech tokens: .NET, C#, C++, getUserName().
    t = sub(re!(r"(?<![\w.])([A-Za-z]*)\.NET\b"), &t, |m| {
        if m.g(1).is_empty() {
            "dot net".to_string()
        } else {
            format!("{} dot net", spell(m.g(1)))
        }
    });
    t = tpl(re!(r"\b([A-Za-z])#(?!\w)"), &t, r"\1 sharp");
    t = tpl(re!(r"\b([A-Za-z])\+\+(?!\w)"), &t, r"\1 plus plus");
    t = sub(re!(r"\b([A-Za-z_][A-Za-z0-9_]*)\(\)"), &t, |m| {
        split_ident(m.g(1))
    });
    t = sub(re!(r"\b[a-z]{2,}(?:[A-Z][a-z]+){2,}\b"), &t, |m| {
        split_ident(m.g(0))
    });
    t = sub(re!(r"\bv(\d+(?:\.\d+)+)\b"), &t, |m| {
        format!(
            "version {}",
            m.g(1).split('.').collect::<Vec<_>>().join(" point ")
        )
    });
    // Hashtags and handles.
    t = sub(
        re!(r"(?<![\w&#])#([A-Za-z][A-Za-z_]*)\b(?![\d-])"),
        &t,
        |m| format!("hashtag {}", split_ident(m.g(1))),
    );
    t = sub(
        re!(r"(?<![\w.@])@([A-Za-z_][A-Za-z0-9_]{1,30})\b"),
        &t,
        |m| format!("at {}", handle(m.g(1))),
    );
    // UK and Canadian postcodes: SW1A 1AA, M14 5TG, K1A 0B6.
    t = sub(
        re!(r"\b([A-Z]{1,2}\d[A-Z\d]?|[A-Z]\d[A-Z])\s(\d[A-Z]{2}|\d[A-Z]\d)\b"),
        &t,
        |m| format!("{}, {}", spell(m.g(1)), spell(m.g(2))),
    );
    // Digits read one by one: card ending in 4459, PIN 0420.
    t = sub(
        re!(
            r"\b((?:ending|ends) in|last (?:four|4)(?: digits)?(?: of)?|PIN|[Pp]in|passcode|OTP|[Cc]ode)(\s+)(\d{3,8}(?:\s\d{2,4})*)\b(?![.,]\d|-\w)"
        ),
        &t,
        |m| {
            let groups: Vec<String> = m.g(3).split_whitespace().map(digits).collect();
            format!("{}{}{}", m.g(1), m.g(2), groups.join(", "))
        },
    );
    // Paths and code: C:\Users, SELECT *, .env, API_KEY.
    t = tpl(re!(r"\b([A-Z]):\\"), &t, r"\1 colon backslash ");
    t = t.replace('\\', " backslash ");
    t = tpl(re!(r"(?<=\s)\*(?=\s)(?!\s*\d)"), &t, "star");
    t = tpl(
        re!(r"(?<![\w.])\.(env|gitignore|bashrc|zshrc|npmrc|htaccess|dockerignore)\b"),
        &t,
        r"dot \1",
    );
    t = sub(
        re!(r"\b(?=\w*_)(?![\w]*\.\w)[A-Za-z0-9]+(?:_[A-Za-z0-9]+)+\b"),
        &t,
        |m| {
            m.g(0)
                .split('_')
                .map(|p| {
                    let has_digit = p.chars().any(|c| c.is_ascii_digit());
                    let has_letter = p.chars().any(|c| c.is_ascii_alphabetic());
                    if has_digit && has_letter {
                        spell(p)
                    } else {
                        p.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join(" underscore ")
        },
    );
    // US toll-free and long-distance: 1-800-829-1040 → one eight hundred, …
    t = sub(
        re!(r"(?<![\w+])1[-. ](\d{3})[-. ](?:(\d{3})[-. ](\d{4})|([A-Z]{3,}(?:-[A-Z]+)*))\b"),
        &t,
        |m| {
            let area = if matches!(m.g(1), "800" | "900") {
                cardinal(int(m.g(1)))
            } else {
                digits(m.g(1))
            };
            let rest = if m.g(2).is_empty() {
                m.g(4)
                    .split('-')
                    .map(capitalize)
                    .collect::<Vec<_>>()
                    .join(" ")
            } else {
                format!("{}, {}", digits(m.g(2)), digits(m.g(3)))
            };
            format!("one {area}, {rest}")
        },
    );
    // Money: currency codes and symbols, ranges, scale words.
    t = sub(
        re_dyn!(format!(r"(?<![\w$])({})\s?{NUM}{SC}", currency_codes())),
        &t,
        |m| currency(m.g(1), m.g(2), m.g(3)),
    );
    t = sub(
        re_dyn!(format!(
            r"{NUM}{SC}\s?(USD|EUR|GBP|JPY|CNY|INR|CAD|AUD|CHF|NZD|HKD|SGD|KRW|MXN|BRL|RUB|SEK|NOK|DKK|ZAR|BTC|ETH)\b"
        )),
        &t,
        |m| currency(m.g(3), m.g(1), m.g(2)),
    );
    t = tpl(re!(r"(?<!\w)(\d+)\s?¢"), &t, r"\1 cents");
    t = sub(
        re_dyn!(format!(r"([$€£]){NUM}{SC}\s?[-–]\s?[$€£]?{NUM}{SC}")),
        &t,
        |m| {
            let (c, a, s1, b, s2) = (m.g(1), m.g(2), m.g(3), m.g(4), m.g(5));
            if s1.is_empty() && s2.is_empty() && search(re!(r"\.\d\d\b"), &format!("{a} {b}")) {
                return format!("{c}{a} to {c}{b}");
            }
            let word = |s: &str| {
                if s.is_empty() {
                    String::new()
                } else {
                    format!(" {}", scale(s.trim()))
                }
            };
            let (w1, w2) = (word(s1), word(s2));
            let lead = if w1 == w2 { "" } else { w1.as_str() };
            format!("{a}{lead} to {b}{w2} {}", money_word(c))
        },
    );
    t = sub(
        re_dyn!(format!(
            r"([$€£]){NUM}(\s?(?:thousand|million|billion|trillion)\b|(?:k|K|mn|m|M|bn|b|B|tn|T)\b)"
        )),
        &t,
        |m| format!("{} {} {}", m.g(2), scale(m.g(3).trim()), money_word(m.g(1))),
    );
    // Durations: 2h 15m, 1h30, 6m 30s; a bare 45m after for/last/takes is minutes.
    t = sub(
        re!(r"\b(\d+)\s?h(?:rs?)?\s?(\d{1,2})(?:\s?m(?:ins?)?)?\b"),
        &t,
        |m| format!("{} {}", plural(m.g(1), "hour"), plural(m.g(2), "minute")),
    );
    t = sub(
        re!(r"\b(\d+)\s?m(?:ins?)?\s?(\d{1,2})\s?s(?:ecs?)?\b"),
        &t,
        |m| format!("{} {}", plural(m.g(1), "minute"), plural(m.g(2), "second")),
    );
    t = sub(
        re!(
            r"(\b(?:for|last|lasts|lasted|lasting|takes?|took|taking|every|within|wait|waited|remaining|runtime|ETA|blocked)\s+(?:\w+\s+){0,2}?)(\d{1,3})m\b(?!\s*(?:dash|sprint|race|swim|hurdles|pool|freestyle|relay))"
        ),
        &t,
        |m| format!("{}{}", m.g(1), plural(m.g(2), "minute")),
    );
    // Decades: '80s, 1990s, the 2000s; '98 → ninety eight.
    t = sub(
        re!(r"(?<![\w$€£.,'])(')?(\d{2}|1\d{3}|20\d{2})'?s\b"),
        &t,
        decade,
    );
    t = sub(re!(r"(?:(?<=\s)|^)'(\d{2})\b(?!['\w])"), &t, |m| {
        if m.g(1).starts_with('0') {
            format!("oh {}", ONES[int(m.g(1)) as usize])
        } else {
            m.g(1).to_string()
        }
    });
    // Roman numerals by context: Henry VIII, Chapter XIV.
    t = sub(re!(r"\bWW\s?(II|I|2|1)\b"), &t, |m| {
        format!(
            "World War {}",
            if matches!(m.g(1), "II" | "2") {
                "Two"
            } else {
                "One"
            }
        )
    });
    t = sub(
        re_dyn!(format!(
            r"\b([A-Za-z][A-Za-z']*\.?)(\s+)({ROMAN_RE})\b(?!'\w|-\d)"
        )),
        &t,
        roman_sub,
    );
    t = sub(re!(r"(?<=[a-z])\((I|II|III|IV|V|VI)\)"), &t, |m| {
        format!(" {}", cardinal(roman(m.g(1))))
    });
    // Powers: x^2 → x squared, 6.02 x 10^23 → 6.02 times 10 to the …
    t = tpl(re!(r"(\d)\s?[x×*]\s?(?=10\s?\^)"), &t, r"\1 times ");
    t = sub(
        re_dyn!(format!(r"{UNIT_PRE}\s?(km|m|cm|mm|ft|mi|yd|in)\^([23])\b")),
        &t,
        |m| {
            format!(
                "{} {} {}",
                m.g(1),
                if m.g(3) == "2" { "square" } else { "cubic" },
                unit_name(m.g(2))
            )
        },
    );
    t = sub(
        re!(r"(\b[A-Za-z]{1,3}|\b\d+(?:\.\d+)?|\))\s?\^\s?\(?([+-]?(?:\d+|[a-z]))\)?"),
        &t,
        power,
    );
    if search(re!(r"[=^+]|\bsolve\b|\bequation"), &t) {
        // 3x - 2 = 11 → 3 x minus 2.
        t = tpl(re!(r"(?<=[\w)])\s+[-–]\s+(?=[\w(])"), &t, " minus ");
        t = tpl(re!(r"\b(\d+)([a-fi-jn-rt-z])\b"), &t, r"\1 \2");
    }
    // Multipliers: 3x faster, about 3x what it cost, grew 3x. → three times.
    t = tpl(
        re!(
            r"\b(\d+(?:\.\d+)?)[xX](?=\s+(?:faster|slower|more|less|larger|smaller|bigger|higher|lower|cheaper|better|worse|longer|shorter|the|as|over|speed|zoom|what|that|this|these|those|its?|their|his|her|our|your|my|an?|in|on|per|than|from|since|normal|usual|before|compared)\b|[.,;:!?)]+(?:\s|$)|$)"
        ),
        &t,
        r"\1 times",
    );
    // Sizes: 4x4, 1920x1080 → four by four (0x1F stays hex).
    t = tpl(re!(r"\b([1-9]\d{0,4})[xX](\d{1,5})\b"), &t, r"\1 by \2");
    // Any other 10x before a word is said "ten x" (a 10x engineer).
    t = tpl(re!(r"\b(\d+(?:\.\d+)?)[xX](?=\s+[A-Za-z])"), &t, r"\1 x");
    t = tpl(
        re!(r"(\d)\s+[x×*]\s+(\d+(?:\.\d+)?)(?=\s*=)"),
        &t,
        r"\1 times \2",
    );
    // Bare scales: 4.5M visitors → 4.5 million visitors (5K run stays).
    sub(
        re!(
            r"(?<![\w$€£.])(\d+\.\d+|\d+(?=\s?\w+\s+[a-z]{3,}s\b))\s?(M|bn|mn|tn)\b(?=\s+[a-z]{3,})"
        ),
        &t,
        |m| format!("{} {}", m.g(1), scale(m.g(2))),
    )
}

// MARK: - Second pass: structured text

fn dotted(s: &str) -> String {
    s.replace('.', " dot ").replace('-', " dash ")
}

fn street_rule(m: &M) -> String {
    let (num, letter, name, kind, dot) = (m.g(1), m.g(2), m.g(3), m.g(4), m.g(5));
    let tail = if !dot.is_empty() && search(re!(r"^\s*(?:$|[A-Z\n])"), m.after()) {
        "."
    } else {
        ""
    };
    let letter = if letter.is_empty() {
        String::new()
    } else {
        format!(" {}", letter.to_uppercase())
    };
    format!(
        "{}{letter} {name} {}{tail}",
        nread(num),
        lookup(STREET, kind).unwrap_or(kind)
    )
}

fn phone(m: &M) -> String {
    let s = m.g(0);
    if s.chars().filter(char::is_ascii_digit).count() < 7 {
        return m.whole();
    }
    let lead = if s.trim().starts_with('+') {
        "plus "
    } else {
        ""
    };
    let mut groups = Vec::new();
    let mut current = String::new();
    for c in s.chars() {
        if c.is_numeric() {
            current.push(c);
        } else if !current.is_empty() {
            groups.push(digits(&current));
            current.clear();
        }
    }
    if !current.is_empty() {
        groups.push(digits(&current));
    }
    format!(" {lead}{} ", groups.join(", "))
}

fn ord_suffix(n: u128) -> String {
    let suffix = if (10..=20).contains(&(n % 100)) {
        "th"
    } else {
        match n % 10 {
            1 => "st",
            2 => "nd",
            3 => "rd",
            _ => "th",
        }
    };
    format!("{n}{suffix}")
}

/// Day/month (or month/day) without a year: 14/03 → the 14th of March,
/// 03/14 → March 14th; ambiguous → day/month.
fn day_month(m: &M, order: DateOrder) -> String {
    let (a, b) = (int(m.g(1)), int(m.g(2)));
    let after = first_chars(m.after(), 12).to_lowercase();
    let before = last_chars(m.before(), 16);
    if search(re!(r"^\s*(stars?|points?|marks?|votes?)\b"), &after)
        || search(
            re!(r"(rated|scored?|rating|got)\s*$"),
            &before.to_lowercase(),
        )
    {
        return format!("{a} out of {b}");
    }
    if 0 < a
        && a < b
        && FRAC_DEN.contains(&b)
        && !m.g(1).starts_with('0')
        && !m.g(2).starts_with('0')
        && (!search(re!(DATE_BEFORE), before) || search(re!(FRAC_AFTER), &after))
    {
        return frac_words(a, b, false);
    }
    let the = if search(re!(r"(?i)\bthe\s+$"), before) {
        ""
    } else {
        "the "
    };
    let day_first = (1..=12).contains(&b) && (1..=31).contains(&a);
    let month_first = (1..=12).contains(&a) && (1..=31).contains(&b);
    if month_first && (order == DateOrder::MonthFirst || !day_first) {
        return format!("{} {}", MONTHS[(a - 1) as usize], ord_suffix(b));
    }
    if day_first {
        return format!("{the}{} of {}", ord_suffix(a), MONTHS[(b - 1) as usize]);
    }
    m.whole()
}

/// Not preceded by a code word (codes stay letter by letter, digit by digit).
fn not_code_context(m: &M) -> bool {
    !search(re!(CODE_CTX), last_chars(m.before(), 40))
}

#[allow(clippy::too_many_lines)]
fn norm_pre(text: &str, order: DateOrder) -> String {
    let mut t = norm_tokens(text);
    // Map scales and large ratios: 1:25,000 and 1:100000 can't be clock times.
    t = sub(
        re!(r"(?<![\d:.,])\b(\d{1,3}):(\d{1,3}(?:,\d{3})+|\d{4,})\b(?![:.,]\d)"),
        &t,
        |m| {
            format!(
                "{} to {}",
                cardinal(int(m.g(1))),
                cardinal(int(&m.g(2).replace(',', "")))
            )
        },
    );
    // Host ports: localhost:8080 → localhost colon eighty eighty.
    t = sub(
        re!(r"\b(localhost|(?:[A-Za-z0-9-]+\.)+[A-Za-z]{2,}):(\d{2,5})\b"),
        &t,
        |m| format!("{} colon {}", m.g(1), nread(m.g(2))),
    );
    // URL schemes: a bare one is read out, http(s) before an address is
    // dropped as people do reading aloud, other schemes are read.
    t = sub(
        re!(r#"(?<![\w.+-])([A-Za-z][A-Za-z0-9+.-]{1,9})://(?=[\s,.;:!?)"']|$)"#),
        &t,
        |m| format!("{} colon slash slash", scheme_say(m.g(1))),
    );
    t = sub(
        re!(
            r"(?<![\w.+-])(?!https?://)([A-Za-z][A-Za-z0-9+.-]{1,9})://(?=[\w/@\[])((?![\w.-]*@)[A-Za-z][\w-]*(?:\.[\w-]+)+\b)?"
        ),
        &t,
        |m| {
            format!(
                "{} colon slash slash {}",
                scheme_say(m.g(1)),
                dotted(m.g(2))
            )
        },
    );
    t = tpl(re!(r"\bmailto:(?=[\w.+-]+@)"), &t, "mail to ");
    // E-mail and web addresses.
    t = sub(re!(r"\b([\w.+-]+)@([\w-]+(?:\.[\w-]+)+)\b"), &t, |m| {
        format!(
            "{} at {}",
            m.g(1)
                .replace('.', " dot ")
                .replace('_', " underscore ")
                .replace('-', " dash "),
            dotted(m.g(2))
        )
    });
    t = sub(
        re!(r"\bhttps?://(?![\w.-]*[:@])([A-Za-z][\w-]*(?:\.[\w-]+)+\b)?"),
        &t,
        |m| {
            let host = m.g(1);
            if !host.is_empty() && !search(re_dyn!(format!(r"\.(?:{DOMAIN_TLD})$")), host) {
                let read = dotted(host);
                match read.strip_prefix("www ") {
                    Some(rest) => format!("w w w {rest}"),
                    None => read,
                }
            } else {
                host.to_string()
            }
        },
    );
    t = tpl(re!(r"\bwww\."), &t, "w w w dot ");
    t = sub(
        re_dyn!(format!(r"\b((?:[\w-]+\.)+)({DOMAIN_TLD})\b")),
        &t,
        |m| format!("{}{}", dotted(m.g(1)), m.g(2)),
    );
    // File names: name_with_parts.ext.
    t = sub(
        re!(
            r"\b([\w-]+)\.(pdf|png|jpe?g|gif|docx?|xlsx?|csv|txt|zip|mp3|mp4|wav|json|py|js|html?|gradle|ipynb|rtf|pptx?|webm|bin|dat|bat|ps1|vue|jsx|kts|cs|dart|lua|r)\b"
        ),
        &t,
        |m| format!("{} dot {}", m.g(1).replace('_', " underscore "), m.g(2)),
    );
    t = sub(
        re!(
            r"(?<![\w@])((?:/?[\w-]+/)*[\w-]*[a-z_][\w-]*)\.(conf|log|cfg|ini|yaml|yml|toml|md|sh|ts|tsx|css|xml|sql|db|bak|tmp|exe|dmg|apk|iso|tar|gz|rar|7z|svg|webp|heic|mov|avi|mkv|flac|ogg|m4a|key|pem|crt|env|lock|rs|go|java|kt|swift|rb|php|c|cpp|h|git)\b"
        ),
        &t,
        |m| {
            let base = m
                .g(1)
                .replace('_', " underscore ")
                .replace('/', " slash ")
                .replace('-', " dash ")
                .replace('.', " dot ");
            format!("{base} dot {}", m.g(2))
        },
    );
    t = sub(re!(r"(?<![\w])/((?:[\w.-]+/)*[\w.-]+)"), &t, |m| {
        format!(" slash {}", m.g(1).replace('/', " slash "))
    });
    // 12-25-1999 and 14.03.2027 are dates; slashed form for the date rules.
    t = tpl(
        re!(r"\b(\d{1,2})-(\d{1,2})-((?:19|20)\d\d)\b"),
        &t,
        r"\1/\2/\3",
    );
    t = sub(re!(r"\b(\d{1,2})\.(\d{1,2})\.((?:19|20)\d\d)\b"), &t, |m| {
        let (d, mo) = (int(m.g(1)), int(m.g(2)));
        if (1..=31).contains(&d) && (1..=12).contains(&mo) {
            format!(
                "the {} of {} {}",
                ordinal(d),
                MONTHS[(mo - 1) as usize],
                m.g(3)
            )
        } else {
            m.whole()
        }
    });
    // Day and month ranges / abbreviations.
    t = sub(
        re_dyn!(format!(
            r"\b({days})\.?\s*[-–]\s*({days})\.?",
            days = keys(DAYS)
        )),
        &t,
        |m| {
            format!(
                "{} to {}",
                lookup(DAYS, m.g(1)).unwrap_or(""),
                lookup(DAYS, m.g(2)).unwrap_or("")
            )
        },
    );
    t = sub(
        re_dyn!(format!(
            r"(?<!\bthe )(?<!\bThe )\b({})\.(?=[\s,]|$)",
            keys(DAYS)
        )),
        &t,
        |m| {
            let dot = if search(re!(r"^\s*(?:$|[A-Z])"), m.after()) {
                "."
            } else {
                ""
            };
            format!("{}{dot}", lookup(DAYS, m.g(1)).unwrap_or(""))
        },
    );
    t = sub(
        re_dyn!(format!(
            r"\b(on|by|until|till|every|from|next|this|last) ({})\b(?!')",
            keys(DAYS)
        )),
        &t,
        |m| format!("{} {}", m.g(1), lookup(DAYS, m.g(2)).unwrap_or("")),
    );
    t = sub(
        re_dyn!(format!(r"\b({})\.(?=\s*\d)", keys(MON_ABBR))),
        &t,
        |m| lookup(MON_ABBR, m.g(1)).unwrap_or("").to_string(),
    );
    t = sub(
        re_dyn!(format!(r"\b({})\b(?=\s+\d{{1,4}}\b(?!:))", keys(MON_ABBR))),
        &t,
        |m| lookup(MON_ABBR, m.g(1)).unwrap_or("").to_string(),
    );
    t = sub(
        re_dyn!(format!(r"(?<=\d )({})\b\.?", keys(MON_ABBR))),
        &t,
        |m| lookup(MON_ABBR, m.g(1)).unwrap_or("").to_string(),
    );
    // House numbers and street types: 742 Evergreen Terrace, 4 Elm Rd.
    t = sub(
        re_dyn!(format!(
            r"\b(\d{{1,4}})([A-Za-z]?)\s+((?:(?:North|South|East|West|N\.?|S\.?|E\.?|W\.?)\s+)?(?:[A-Z][\w']*|\d+(?:st|nd|rd|th))(?:\s+[A-Z][\w']*){{0,2}})\s+(Street|Road|Avenue|Boulevard|Lane|Drive|Court|Place|Square|Highway|Parkway|Crescent|Terrace|Circle|Way|Close|Row|Walk|Alley|Plaza|Trail|Pike|Loop|{})\b(\.?)",
            keys(STREET)
        )),
        &t,
        street_rule,
    );
    t = sub(
        re!(
            r"(?<!Mount )(?<!Port )(?<!Fort )(?<!Lake )(?<!Cape )\b([A-Z][a-z]+|\d+(?:st|nd|rd|th))\s+(St|Rd|Ave|Blvd|Ln|Hwy|Pkwy|Cres|Terr?)\b(\.?)"
        ),
        &t,
        |m| {
            let lowered = m.g(1).to_lowercase();
            if ROMAN_STOP.contains(&lowered.as_str())
                || matches!(lowered.as_str(), "on" | "at" | "visit" | "see")
                || m.g(2) == "St" && search(re!(r"^\s+[A-Z]"), m.after())
            {
                return m.whole();
            }
            let dot = if !m.g(3).is_empty() && search(re!(r"^\s*(?:$|\n)"), m.after()) {
                "."
            } else {
                ""
            };
            format!("{} {}{dot}", m.g(1), lookup(STREET, m.g(2)).unwrap_or(""))
        },
    );
    t = sub(
        re!(r"\b((?:I|US|SR|Route|Highway|Hwy|Interstate)[- ]\d+)\s([NSEW])\b"),
        &t,
        |m| {
            let way = match m.g(2) {
                "N" => "North",
                "S" => "South",
                "E" => "East",
                _ => "West",
            };
            format!("{} {way}", m.g(1))
        },
    );
    // "Bus no. 42" is said "bus forty two". Phonon also misreads a sentence
    // that starts "Bus number" as "Buzz number".
    t = tpl(re!(r"\b([Bb]us|[Rr]oute)\s+[Nn]o\.\s*(?=\d)"), &t, r"\1 ");
    static ABBR_RE: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
        ABBR.iter()
            .map(|(p, r)| (Regex::new(p).expect("valid normalizer regex"), *r))
            .collect()
    });
    for (re, replacement) in ABBR_RE.iter() {
        t = tpl(re, &t, replacement);
    }
    // ISO dates: 2026-10-21 → October 21, 2026.
    t = sub(re!(r"\b(\d{4})-(\d{2})-(\d{2})\b"), &t, |m| {
        let mo = int(m.g(2));
        if (1..=12).contains(&mo) {
            format!("{} {}, {}", MONTHS[(mo - 1) as usize], int(m.g(3)), m.g(1))
        } else {
            m.whole()
        }
    });
    // 3-17 November → the third to the seventeenth of November.
    t = sub(
        re_dyn!(format!(
            r"\b(\d{{1,2}})(?:st|nd|rd|th)?\s*[-–]\s*(\d{{1,2}})(?:st|nd|rd|th)?\s+{MON_RE}\b"
        )),
        &t,
        |m| {
            let (a, b) = (int(m.g(1)), int(m.g(2)));
            if a < b && b <= 31 {
                format!("the {} to the {} of {}", ordinal(a), ordinal(b), m.g(3))
            } else {
                m.whole()
            }
        },
    );
    // 14 March 2027 → the fourteenth of March 2027.
    t = sub(
        re_dyn!(format!(
            r"\b(\d{{1,2}})(?:st|nd|rd|th)?\s+(?:of\s+)?{MON_RE}\b"
        )),
        &t,
        |m| {
            let d = int(m.g(1));
            if !(1..=31).contains(&d) {
                return m.whole();
            }
            let the = if search(re!(r"(?i)\bthe\s+$"), m.before()) {
                ""
            } else {
                "the "
            };
            format!("{the}{} of {}", ordinal(d), m.g(2))
        },
    );
    // Year ranges 1968-2020, 1990-95 (before the phone rule can take them).
    t = sub(
        re!(r"\b(1[5-9]\d\d|20\d\d)\s*[-–]\s*(1[5-9]\d\d|20\d\d|\d\d)\b(?![-/.]\d)"),
        &t,
        |m| {
            let tail = int(last_chars(m.g(1), 2));
            if count(m.g(2)) == 4 || int(m.g(2)) > tail {
                format!("{} to {}", m.g(1), m.g(2))
            } else {
                m.whole()
            }
        },
    );
    // Month/year 3/2027 → March 2027; set phrases 24/7, 50/50, 9/11.
    t = sub(re!(r"(?<![\d/])\b(\d{1,2})/((?:19|20)\d\d)\b"), &t, |m| {
        let mo = int(m.g(1));
        if (1..=12).contains(&mo) {
            format!("{} {}", MONTHS[(mo - 1) as usize], m.g(2))
        } else {
            m.whole()
        }
    });
    t = sub(re!(r"(?<![\d/])\b(24/7|50/50|9/11)\b(?!/)"), &t, |m| {
        match m.g(1) {
            "24/7" => "24 7",
            "50/50" => "fifty fifty",
            _ => "nine eleven",
        }
        .to_string()
    });
    // Fractions: 1 1/2 → 1 and a half (dates need a date word before them).
    t = sub(
        re!(r"(?<![\d/.,])\b(\d+)\s+(\d{1,2})/(\d{1,2})\b(?!/\d)"),
        &t,
        |m| {
            let (a, b) = (int(m.g(2)), int(m.g(3)));
            if 0 < a && a < b && FRAC_DEN.contains(&b) {
                format!("{} and {}", m.g(1), frac_words(a, b, true))
            } else {
                m.whole()
            }
        },
    );
    t = sub(re!(r"(?<![\d/])\b(\d{1,2})/(\d{1,2})\b(?!/\d)"), &t, |m| {
        day_month(m, order)
    });
    // Multi-dot versions 4.0.1(-beta).
    t = sub(
        re!(r"\b(\d+(?:\.\d+){2,})(?:-(alpha|beta|rc\d*|preview))?\b"),
        &t,
        |m| {
            let read = m.g(1).split('.').collect::<Vec<_>>().join(" point ");
            if m.g(2).is_empty() {
                read
            } else {
                format!("{read} {}", m.g(2))
            }
        },
    );
    // 2:03:45 → two hours three minutes … (a clock time after "at" keeps its
    // seconds at the end).
    t = sub(re!(r"\b(\d{1,2}):(\d{2}):(\d{2})\b(?!:)"), &t, |m| {
        if search(re!(r"\b(at|by|until|from)\s+$"), m.before()) {
            return format!("{}:{} and {}", m.g(1), m.g(2), plural_int(m.g(3), "second"));
        }
        let mut parts = Vec::new();
        for (value, word) in [(m.g(1), "hour"), (m.g(2), "minute"), (m.g(3), "second")] {
            if int(value) != 0 || word == "second" {
                parts.push(plural_int(value, word));
            }
        }
        parts.join(", ")
    });
    // Time ranges: 10:00-16:30, 8 AM-6 PM, 9am-5pm.
    t = tpl(
        re!(
            r"\b(\d{1,2}(?::\d{2})?\s?(?:[AaPp]\.?[Mm]\.?)?)\s?[-–]\s?(\d{1,2}(?::\d{2})?\s?[AaPp]\.?[Mm]\b\.?)"
        ),
        &t,
        r"\1 to \2",
    );
    t = tpl(
        re!(r"\b(\d{1,2}:\d{2})\s?[-–]\s?(\d{1,2}:\d{2})\b"),
        &t,
        r"\1 to \2",
    );
    // Ratios and scores with a one-digit right side: 3:2; 16:9 after "ratio".
    t = tpl(re!(r"\b(\d{1,2}):(\d)\b(?!\d)"), &t, r"\1 to \2");
    t = tpl(
        re!(r"(?<=[Rr]atio of |[Rr]atio is )(\d{1,2}):(\d{1,2})\b"),
        &t,
        r"\1 to \2",
    );
    // Letter-led codes with digit groups, before the phone rule takes them.
    t = sub(
        re!(r"\b[A-Z]{1,4}(?:-[A-Z0-9]{1,6})*-\d[A-Za-z0-9]*(?:-[A-Za-z0-9]+)*\b"),
        &t,
        |m| code_read(m.g(0)),
    );
    // Phone numbers: optional +CC, digit groups, 7+ digits in total.
    t = sub(
        re!(r"(?<![\w.,])\+?\(?\d{2,4}\)?(?:[\s.-]\(?\d{2,4}\)?){1,4}(?![\w.,]*\d)"),
        &t,
        phone,
    );
    // Short service numbers after call/dial/text.
    t = sub(
        re!(r"\b((?:[Cc]all|[Dd]ial|[Tt]ext|[Rr]ing)\s+)(\d{3,6})\b"),
        &t,
        |m| format!("{}{}", m.g(1), digits(m.g(2))),
    );
    // Units after numbers (singular after exactly 1; 10 mg/kg → per kilogram).
    t = sub(re!(r"\b(\d+)\s?ft\.?\s?(\d{1,2})\s?in\b\.?"), &t, |m| {
        format!(
            "{} {} {} {}",
            m.g(1),
            if m.g(1) == "1" { "foot" } else { "feet" },
            m.g(2),
            if m.g(2) == "1" { "inch" } else { "inches" }
        )
    });
    t = tpl(re!(r#"\b(\d)'\s?(\d{1,2})(?:"|'')"#), &t, r"\1 foot \2");
    let per_unit = |m: &M| {
        let per = if m.g(3) == "s" {
            sing("seconds")
        } else {
            sing(unit_name(m.g(3)))
        };
        format!("{} {} per {per}", m.g(1), unit(m.g(2), m.g(1)))
    };
    t = sub(
        re_dyn!(format!(
            r"{UNIT_PRE}\s?({keys})/({keys}|s|in)(?![\w/])",
            keys = ukeys()
        )),
        &t,
        |m| {
            if m.g(3) == "in" {
                m.whole()
            } else {
                per_unit(m)
            }
        },
    );
    t = sub(
        re_dyn!(format!(r"{UNIT_PRE}\s?({})(?![\w/'])", ukeys())),
        &t,
        |m| format!("{} {}", m.g(1), unit(m.g(2), m.g(1))),
    );
    t = sub(
        re_dyn!(format!(
            r"(\b(?:squared|cubed|to the (?:minus )?(?:\w+ )?\w+(?:th|st|nd|rd))\s)({keys})(?:/({keys}|s))?(?![\w/'])",
            keys = ukeys()
        )),
        &t,
        |m| {
            let per = if m.g(3).is_empty() {
                String::new()
            } else if m.g(3) == "s" {
                format!(" per {}", sing("seconds"))
            } else {
                format!(" per {}", sing(unit_name(m.g(3))))
            };
            format!("{}{}{per}", m.g(1), unit_name(m.g(2)))
        },
    );
    // Identifiers read in pairs: room 402, gate 47B, flight UA 482, B12, A380.
    t = sub(
        re_dyn!(format!(
            r"\b({ID_WORDS})(\s+(?:No\.?\s*|#\s*)?)([A-Z]{{1,3}}\s?)?(\d{{2,4}})([A-Z]{{0,2}})\b(?![.,]\d|\s?[-–]\s?\d)"
        )),
        &t,
        |m| {
            let letters = if m.g(3).is_empty() {
                String::new()
            } else {
                format!("{} ", spell(m.g(3)))
            };
            let tail = if m.g(5).is_empty() {
                String::new()
            } else {
                format!(" {}", spell(m.g(5)))
            };
            format!("{}{}{letters}{}{tail}", m.g(1), m.g(2), nread(m.g(4)))
        },
    );
    t = sub(
        re!(r"\b([A-Z]{2,3})\s(\d{3,4})\b(?![.,:]\d|\s\d)"),
        &t,
        |m| format!("{} {}", m.g(1), nread(m.g(2))),
    );
    t = sub(
        re!(r"(?<![-/.\w])([A-Z]{1,3})(\d{1,4})([A-Z]{0,2})(?![-/\w])"),
        &t,
        |m| {
            if not_code_context(m)
                && (count(m.g(2)) <= 2 || count(m.g(1)) == 2 && m.g(3).is_empty())
            {
                [spell(m.g(1)), nread(m.g(2)), spell(m.g(3))]
                    .into_iter()
                    .filter(|x| !x.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ")
            } else {
                m.whole()
            }
        },
    );
    t = sub(
        re!(r"(?<![-/.\w])(\d{1,4})([A-Z]{1,2})(?![-/\w])"),
        &t,
        |m| {
            if not_code_context(m) {
                format!("{} {}", nread(m.g(1)), spell(m.g(2)))
            } else {
                m.whole()
            }
        },
    );
    // Chemical formulas: C6H12O6 → C six H twelve O six.
    t = sub(
        re!(r"(?<![-/.\w])(?=[A-Za-z]*\d)(?:[A-Z][a-z]?\d{0,3}){2,}(?![-/\w])"),
        &t,
        |m| {
            let element = re!(r"([A-Z][a-z]?)(\d*)");
            let mut parts = Vec::new();
            let mut all_elements = true;
            for caps in element.captures_iter(m.g(0)).flatten() {
                let symbol = caps.get(1).map_or("", |g| g.as_str());
                let amount = caps.get(2).map_or("", |g| g.as_str());
                all_elements &= ELEMENTS.contains(&symbol);
                parts.push(if amount.is_empty() {
                    spell(symbol)
                } else {
                    format!("{} {}", spell(symbol), cardinal(int(amount)))
                });
            }
            if all_elements && not_code_context(m) {
                parts.join(" ")
            } else {
                m.whole()
            }
        },
    );
    // Hash numbers: #1 → number one, #4471 → number four four seven one.
    t = sub(re!(r"#(\d+(?:-\d+)*)\b"), &t, |m| {
        let groups: Vec<String> = m
            .g(1)
            .split('-')
            .map(|g| {
                if count(g) <= 3 {
                    g.to_string()
                } else {
                    digits(g)
                }
            })
            .collect();
        format!("number {}", groups.join(", "))
    });
    // Codes: letter/digit mixes with or without dashes.
    t = sub(
        re!(r"\b(?=[A-Za-z0-9-]*\d)(?=[A-Za-z0-9-]*[A-Za-z])[A-Za-z0-9]+(?:-[A-Za-z0-9]+)+\b"),
        &t,
        |m| code_read(m.g(0)),
    );
    t = sub(re!(r"\b(?=\w*\d)(?=\w*[A-Z])[A-Z0-9]{3,}\b"), &t, |m| {
        spell(m.g(0))
    });
    // Random mixed IDs (4hQx8Z, a9F2k), not ordinals, times or units.
    t = sub(
        re!(r"\b(?=[A-Za-z0-9]*\d)(?=[A-Za-z0-9]*[A-Za-z])[A-Za-z0-9]{4,}\b"),
        &t,
        |m| {
            if search(
                re!(
                    r"(?i)^\d+(?:st|nd|rd|th|am|pm|s|kg|km|mm|cm|ml|mg|gb|mb|tb|kb|ghz|mhz|kw|kwh|mah|hrs?|mins?|h|m|k|x|g|l|w|v)$"
                ),
                m.g(0),
            ) {
                m.whole()
            } else {
                spell(m.g(0))
            }
        },
    );
    // Long bare digit strings (order / account numbers): 5+ digits.
    t = sub(re!(r"(?<![\d$€£.,])\b(\d{5,})\b(?![.,]\d)"), &t, |m| {
        digits(m.g(1))
    });
    // Ranges between small numbers: 9-5, 3-5 days, 0-2.
    t = tpl(
        re!(r"(?<![\d/:.-])\b(\d{1,4})\s*[-–]\s*(\d{1,4})\b(?![/:.-]\d)"),
        &t,
        r"\1 to \2",
    );
    // Durations: 2h 15, 1h30, 45min, 3hrs.
    t = sub(re!(r"\b(\d+)\s*h\s*(\d{1,2})\b"), &t, |m| {
        format!("{} {}", hour_word(m.g(1)), m.g(2))
    });
    t = sub(re!(r"\b(\d+)\s*hrs?\b"), &t, |m| hour_word(m.g(1)));
    t = sub(re!(r"\b(\d+)\s*mins?\b"), &t, |m| plural(m.g(1), "minute"));
    t = sub(
        re_dyn!(format!(r"{UNIT_PRE}\s?({})(?![\w/'])", ukeys())),
        &t,
        |m| format!("{} {}", m.g(1), unit(m.g(2), m.g(1))),
    );
    // Dimensions with ×.
    t = tpl(re!(r"\s*×\s*"), &t, " by ");
    t = tpl(re!(r"(?<=\d)\s+x\s+(?=\d)"), &t, " by ");
    static SYMBOLS_RE: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
        SYMBOLS
            .iter()
            .map(|(p, r)| (Regex::new(p).expect("valid normalizer regex"), *r))
            .collect()
    });
    for (re, replacement) in SYMBOLS_RE.iter() {
        t = tpl(re, &t, replacement);
    }
    // Pounds and pence: £7.50 → 7 pounds 50.
    t = sub(re!(r"£(\d[\d,]*)\.(\d{2})\b"), &t, |m| {
        if m.g(2) == "00" {
            format!("{} pounds", m.g(1))
        } else {
            format!("{} pounds {}", m.g(1), int(m.g(2)))
        }
    });
    // Leftover en dashes aren't in the voice's alphabet.
    t = tpl(re!(r"\s+–\s+"), &t, " — ").replace('–', "-");
    tpl(re!(r"\s{2,}"), &t, " ").trim().to_string()
}

// MARK: - Last pass: remaining digits and symbols

/// Light verbalizer for numbers, money, times and ordinals.
#[allow(clippy::too_many_lines)]
fn fallback(t: &str, order: DateOrder) -> String {
    let mut t = sub(re!(r"([$€£])(\d[\d,]*)(?:\.(\d+))?\b"), t, |m| {
        let (one, many, c1, cn) = match m.g(1) {
            "$" => ("dollar", "dollars", "cent", "cents"),
            "€" => ("euro", "euros", "cent", "cents"),
            _ => ("pound", "pounds", "penny", "pence"),
        };
        let (amount, frac) = (m.g(2), m.g(3));
        if !frac.is_empty() && count(frac) != 2 {
            return format!("{} {many}", number(&format!("{amount}.{frac}")));
        }
        let whole = int(amount);
        let cents = if frac.is_empty() { 0 } else { int(frac) };
        let cent_word = if cents == 1 { c1 } else { cn };
        if whole == 0 && cents != 0 {
            return format!("{} {cent_word}", cardinal(cents));
        }
        let head = format!(
            "{} {}",
            cardinal(whole),
            if whole == 1 { one } else { many }
        );
        if cents == 0 {
            head
        } else {
            format!("{head} and {} {cent_word}", cardinal(cents))
        }
    });
    t = sub(
        re!(r"\b(\d{1,2}):(\d{2})\s*([aApP]\.?[mM])?(?:\.(?=\s+[a-z]))?(?=\W|$)"),
        &t,
        |m| {
            let (h, mi, ampm) = (int(m.g(1)), int(m.g(2)), m.g(3));
            if mi == 0 && ampm.is_empty() && search(re!(r"^\s*(noon|midnight)"), m.after()) {
                return cardinal(h);
            }
            if mi == 0 && ampm.is_empty() && (h > 12 || h == 0) {
                let hour = if h == 0 {
                    "zero".to_string()
                } else {
                    cardinal(h)
                };
                return format!("{hour} hundred");
            }
            let minutes = if mi == 0 {
                if ampm.is_empty() {
                    " o'clock".to_string()
                } else {
                    String::new()
                }
            } else if mi < 10 {
                format!(" oh {}", ONES[mi as usize])
            } else {
                format!(" {}", cardinal(mi))
            };
            let suffix = if ampm.is_empty() {
                String::new()
            } else {
                format!(" {}", ampm.replace('.', "").to_uppercase())
            };
            format!("{}{minutes}{suffix}", cardinal(h))
        },
    );
    t = sub(
        re!(r"\b(\d{1,2})\s*([aApP])\.?[mM](?:\.(?=\s+[a-z]))?(?=\W|$)"),
        &t,
        |m| format!("{} {}M", cardinal(int(m.g(1))), m.g(2).to_uppercase()),
    );
    t = sub(re!(r"\b(\d{1,2})/(\d{1,2})/(\d{4}|\d{2})\b"), &t, |m| {
        let (mo, d) = (int(m.g(1)), int(m.g(2)));
        let y = int(m.g(3)) + if count(m.g(3)) == 2 { 2000 } else { 0 };
        if (mo > 12 || order == DateOrder::DayFirst)
            && (1..=31).contains(&mo)
            && (1..=12).contains(&d)
        {
            return format!(
                "the {} of {} {}",
                ordinal(mo),
                MONTHS[(d - 1) as usize],
                year(y)
            );
        }
        if !((1..=12).contains(&mo) && (1..=31).contains(&d)) {
            return m.whole();
        }
        format!("{} {} {}", MONTHS[(mo - 1) as usize], ordinal(d), year(y))
    });
    t = sub(
        re_dyn!(format!(r"\b({})\.? (\d{{1,2}})\b(?!:)", MONTHS.join("|"))),
        &t,
        |m| format!("{} {}", m.g(1), ordinal(int(m.g(2)))),
    );
    t = tpl(re!(r"(\d)\s?°\s?C\b"), &t, r"\1 degrees Celsius");
    t = tpl(re!(r"(\d)\s?°\s?F\b"), &t, r"\1 degrees Fahrenheit");
    t = tpl(re!(r"(\d)\s?°"), &t, r"\1 degrees");
    t = tpl(re!(r"(?<=\w)/(?=\w)"), &t, " slash ");
    t = sub(re!(r"\b(\d+)(st|nd|rd|th)\b"), &t, |m| ordinal(int(m.g(1))));
    t = sub(re!(r"\b(1[1-9]\d\d|20\d\d)\b(?![.,]\d)"), &t, |m| {
        year(int(m.g(1)))
    });
    t = tpl(re!(r"(?<![\w-])-(?=\d)"), &t, "minus ");
    t = tpl(re!(r"(?<=[A-Za-z])(?=\d)"), &t, " ");
    t = sub(re!(r"(\d+(?:\.\d+)?)\s?%"), &t, |m| {
        format!("{} percent", number(m.g(1)))
    });
    t = sub(
        re!(r"\b\d{1,3}(?:,\d{3})+(?:\.\d+)?\b|\b\d+(?:\.\d+)?\b"),
        &t,
        |m| number(m.g(0)),
    );
    t.replace('&', " and ")
        .replace('+', " plus ")
        .replace('=', " equals ")
        .replace('@', " at ")
        .replace('#', " number ")
}

// MARK: - Respelling

/// `written<TAB>respelling<TAB>flags` (`c` = only when capitalized as written).
const LEXICON: &str = include_str!("../assets/respell.tsv");

struct Lexicon {
    entries: HashMap<String, (String, bool)>,
    max_words: usize,
}

static LEX: LazyLock<Lexicon> = LazyLock::new(|| {
    let mut entries = HashMap::new();
    let mut max_words = 1;
    for line in LEXICON.lines() {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split('\t');
        let (Some(written), Some(spoken)) = (fields.next(), fields.next()) else {
            continue;
        };
        let key = fold(written);
        max_words = max_words.max(key.matches(' ').count() + 1);
        let caps = fields.next().is_some_and(|flags| flags.contains('c'));
        entries.insert(key, (join_syllables(spoken), caps));
    }
    Lexicon { entries, max_words }
});

/// The lexicon spells names in syllables (Bah-gahn), but voices pause at each
/// hyphen and read the parts as separate words, so they get one word (Bahgahn).
fn join_syllables(spoken: &str) -> String {
    spoken
        .split_whitespace()
        .map(|word| {
            let mut chars = word.chars().filter(|c| *c != '-');
            match chars.next() {
                Some(first) => std::iter::once(first)
                    .chain(chars.flat_map(char::to_lowercase))
                    .collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<String>>()
        .join(" ")
}

/// Lookup key: no accents, lower case, straight apostrophes, hyphens as spaces.
fn fold(s: &str) -> String {
    let straight = s.replace(['’', '‘'], "'");
    let bare: String = straight
        .nfkd()
        .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
        .collect();
    let lowered = bare
        .to_lowercase()
        .replace('ł', "l")
        .replace('ø', "o")
        .replace('æ', "ae")
        .replace('œ', "oe")
        .replace('ß', "ss")
        .replace('đ', "d");
    lowered
        .replace(['-', '‐', '–'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// A word joined to another by a hyphen matches alone only when it is a
/// capitalized name and the other part is lower case (Worcestershire-glazed).
fn hyphen_ok(t: &str, a: usize, b: usize, src: &str) -> bool {
    let name_like = src.chars().next().is_some_and(char::is_uppercase);
    let joined_before = t[..a].ends_with('-')
        && t[..a - 1]
            .chars()
            .next_back()
            .is_some_and(char::is_alphabetic);
    if joined_before {
        let other = t[..a - 1].split_whitespace().next_back().unwrap_or("");
        if !(name_like && other.chars().next().is_some_and(char::is_lowercase)) {
            return false;
        }
    }
    let joined_after =
        t[b..].starts_with('-') && t[b + 1..].chars().next().is_some_and(char::is_alphabetic);
    if joined_after {
        let other = t[b + 1..].split_whitespace().next().unwrap_or("");
        if !(name_like && other.chars().next().is_some_and(char::is_lowercase)) {
            return false;
        }
    }
    true
}

/// Respell hard names and foreign words: longest entry wins, whole words only,
/// and a possessive 's survives (Eoin's → Owen's).
fn respell(text: &str) -> String {
    let lexicon = &*LEX;
    if lexicon.entries.is_empty() {
        return text.to_string();
    }
    let t = text.replace('’', "'");
    let words: Vec<(usize, usize)> = re!(r"[^\W\d_]+(?:'[^\W\d_]+)*")
        .find_iter(&t)
        .flatten()
        .map(|m| (m.start(), m.end()))
        .collect();
    let mut out = String::with_capacity(t.len());
    let (mut pos, mut i) = (0, 0);
    while i < words.len() {
        let mut matched = false;
        for n in (1..=lexicon.max_words.min(words.len() - i)).rev() {
            let span = &words[i..i + n];
            if span
                .windows(2)
                .any(|pair| !matches!(&t[pair[0].1..pair[1].0], " " | "-"))
            {
                continue;
            }
            let (a, b) = (span[0].0, span[n - 1].1);
            let src = &t[a..b];
            if !hyphen_ok(&t, a, b, src) {
                continue;
            }
            let key = fold(src);
            let (hit, tail) = if let Some(hit) = lexicon.entries.get(&key) {
                (hit, "")
            } else if let Some(hit) = key
                .strip_suffix("'s")
                .and_then(|stem| lexicon.entries.get(stem))
            {
                (hit, &src[src.len() - 2..])
            } else {
                continue;
            };
            let (spoken, caps) = (hit.0.as_str(), hit.1);
            if caps
                && !span
                    .iter()
                    .all(|&(s, _)| t[s..].chars().next().is_some_and(char::is_uppercase))
            {
                continue;
            }
            out.push_str(&t[pos..a]);
            if src.chars().next().is_some_and(char::is_uppercase)
                && spoken.chars().next().is_some_and(char::is_lowercase)
            {
                out.push_str(&capitalize_first(spoken));
            } else {
                out.push_str(spoken);
            }
            out.push_str(tail);
            pos = b;
            i += n;
            matched = true;
            break;
        }
        if !matched {
            i += 1;
        }
    }
    out.push_str(&t[pos..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> NormalizeOptions {
        NormalizeOptions {
            normalize_text: true,
            respell_names: true,
            date_order: DateOrder::MonthFirst,
        }
    }

    #[test]
    fn reads_structured_text_like_a_person() {
        let cases = [
            (
                "Order #4471 ships 14/03.",
                "Order number four four seven one ships the fourteenth of March.",
            ),
            (
                "The parcel weighs 2.4 kg.",
                "The parcel weighs two point four kilograms.",
            ),
            (
                "Henry VIII had six wives.",
                "Henry the eighth had six wives.",
            ),
            (
                "The temperature is -3.5°C.",
                "The temperature is minus three point five degrees Celsius.",
            ),
        ];
        for (written, spoken) in cases {
            assert_eq!(normalize(written, all()), spoken, "input: {written}");
        }
    }

    #[test]
    fn respelled_names_are_one_word_each() {
        assert_eq!(join_syllables("Bah-gahn"), "Bahgahn");
        assert_eq!(join_syllables("Bug-uh-vud Gee-tah"), "Buguhvud Geetah");
        let names = NormalizeOptions {
            normalize_text: false,
            ..all()
        };
        assert_eq!(normalize("Bhagavad Gita.", names), "Buguhvud Geetah.");
    }

    #[test]
    fn multipliers_and_sizes_are_said_in_words() {
        let cases = [
            (
                "It cost about 3x what it did in 2019.",
                "It cost about three times what it did in twenty nineteen.",
            ),
            ("Sales grew 3x.", "Sales grew three times."),
            (
                "Sales grew 3x, then fell.",
                "Sales grew three times, then fell.",
            ),
            (
                "Prices rose 3X in a year.",
                "Prices rose three times in a year.",
            ),
            ("It's 3X cheaper.", "It's three times cheaper."),
            (
                "That is 2.5x the price.",
                "That is two point five times the price.",
            ),
            ("He is a 10x engineer.", "He is a ten x engineer."),
            ("We saw a 3x improvement.", "We saw a three x improvement."),
            ("Get the 4x4 truck.", "Get the four by four truck."),
            (
                "Set it to 800x600.",
                "Set it to eight hundred by six hundred.",
            ),
            // Hex, file names and codes keep their own rules.
            ("The flag is 0x1F.", "The flag is zero X one F."),
            (
                "The image is logo@2x.png.",
                "The image is logo at 2x dot png.",
            ),
            ("Code 7X-19.", "Code seven X nineteen."),
        ];
        for (written, spoken) in cases {
            assert_eq!(normalize(written, all()), spoken, "input: {written}");
        }
    }

    #[test]
    fn bus_numbers_are_said_like_people_say_them() {
        assert_eq!(
            normalize("Bus no. 42 stops here.", all()),
            "Bus forty two stops here."
        );
        // Other numbers keep the word.
        assert_eq!(normalize("Item no. 42.", all()), "Item number forty two.");
    }

    #[test]
    fn map_scales_are_ratios_not_clock_times() {
        for (written, spoken) in [
            (
                "The scale is 1:25,000.",
                "The scale is one to twenty five thousand.",
            ),
            ("A 1:100000 map.", "A one to one hundred thousand map."),
        ] {
            assert_eq!(normalize(written, all()), spoken);
        }
        // Real clock times and short ratios keep their own rules.
        assert_eq!(
            normalize("Leave at 1:25.", all()),
            "Leave at one twenty five."
        );
    }

    #[test]
    fn one_text_reads_every_date_in_one_order() {
        let day_first = NormalizeOptions {
            date_order: DateOrder::DayFirst,
            ..all()
        };
        let ambiguous = "Shipped on 09/10/2026, arriving 12/10.";
        assert_eq!(
            normalize(ambiguous, all()),
            "Shipped on September tenth twenty twenty six, arriving December tenth."
        );
        assert_eq!(
            normalize(ambiguous, day_first),
            "Shipped on the ninth of October twenty twenty six, arriving the twelfth of October."
        );
        // An unambiguous date settles the order, whatever the setting.
        assert_eq!(
            normalize("From 09/10 to 25/10.", all()),
            "From the ninth of October to the twenty fifth of October."
        );
        assert_eq!(
            normalize("From 09/10 to 10/25.", day_first),
            "From September tenth to October twenty fifth."
        );
    }

    #[test]
    fn prose_passes_through_unchanged() {
        let prose = "The old harbor town woke slowly, as if told there was no reason to hurry.";
        assert_eq!(normalize(prose, all()), prose);
    }

    #[test]
    fn respelling_keeps_possessive_and_respects_settings() {
        assert_eq!(
            normalize(
                "Siobhan's book.",
                NormalizeOptions {
                    normalize_text: false,
                    ..all()
                }
            ),
            "Shivawn's book."
        );
        let off = NormalizeOptions {
            normalize_text: false,
            respell_names: false,
            ..all()
        };
        assert_eq!(normalize("Siobhan paid $5.", off), "Siobhan paid $5.");
        let no_names = NormalizeOptions {
            respell_names: false,
            ..all()
        };
        assert_eq!(
            normalize("Siobhan paid $5.", no_names),
            "Siobhan paid five dollars."
        );
    }

    #[test]
    fn hostile_input_never_panics() {
        for text in [
            "",
            "   ",
            "٣٤٥ ۱۲۳ १२३ 12/٣",
            "99999999999999999999999999999999999999999999999999999999",
            "#99999999999999999999999999999999999999999999 $999999999999999999999999999999999999999999.99",
            "é-Siobhan-é -Siobhan- Siobhan- -",
            "32/13 13/32 00/00 0:00:00 99:99:99 1/0 0/1 99-99-2026 2026-99-99",
            "MMMMCMXCIX Henry MMM ^^^ 2^ x^ ^2 10^999",
            "½¼¾⅓⅔⅛⁰¹²³ ⁻⁻ ¹²³⁴⁵⁶⁷⁸⁹",
        ] {
            let _ = normalize(text, all());
        }
    }

    #[test]
    fn window_slices_stay_on_char_boundaries() {
        // Multi-byte chars around the 12/16/40-char context windows.
        let text = format!("{} 3/4 {}", "é".repeat(50), "ü".repeat(50));
        let _ = normalize(&text, all());
        let code = format!("{} code {} A12 {}", "日本".repeat(30), "é".repeat(30), "é");
        let _ = normalize(&code, all());
    }
}
