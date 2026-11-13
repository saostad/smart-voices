//! Persian text expansions for numbers with punctuation, done before the
//! Persian normalizer turns digits into words. Pure: no I/O, no model.
//!
//! A "digit" is ASCII `0-9`, Persian `۰-۹` or Arabic `٠-٩`. Numbers this
//! module rewrites are written back as ASCII digits without leading zeros.
//! A pattern only matches when it is not glued to another digit on either
//! side. Text that matches no rule is returned unchanged.

const JALALI_MONTHS: [&str; 12] = [
    "فروردین",
    "اردیبهشت",
    "خرداد",
    "تیر",
    "مرداد",
    "شهریور",
    "مهر",
    "آبان",
    "آذر",
    "دی",
    "بهمن",
    "اسفند",
];

const GREGORIAN_MONTHS: [&str; 12] = [
    "ژانویه",
    "فوریه",
    "مارس",
    "آوریل",
    "مه",
    "ژوئن",
    "ژوئیه",
    "اوت",
    "سپتامبر",
    "اکتبر",
    "نوامبر",
    "دسامبر",
];

fn digit_value(c: char) -> Option<u32> {
    match c {
        '0'..='9' => Some(u32::from(c) - u32::from('0')),
        '۰'..='۹' => Some(u32::from(c) - u32::from('۰')),
        '٠'..='٩' => Some(u32::from(c) - u32::from('٠')),
        _ => None,
    }
}

fn is_digit(c: char) -> bool {
    digit_value(c).is_some()
}

/// A maximal digit run starting at `start`: its end index, its value, and
/// whether it has a leading zero (more than one digit, the first zero).
struct DigitRun {
    end: usize,
    value: u64,
    leading_zero: bool,
}

fn digit_run(chars: &[char], start: usize) -> Option<DigitRun> {
    if start >= chars.len() || !is_digit(chars[start]) {
        return None;
    }
    let mut end = start;
    let mut value: u64 = 0;
    while end < chars.len() {
        let Some(digit) = digit_value(chars[end]) else {
            break;
        };
        value = value.saturating_mul(10).saturating_add(u64::from(digit));
        end += 1;
    }
    Some(DigitRun {
        end,
        value,
        leading_zero: end - start > 1 && digit_value(chars[start]) == Some(0),
    })
}

/// The run's digits as ASCII, without leading zeros (`"0"` for zero itself).
fn normalized_digits(chars: &[char], start: usize, end: usize) -> String {
    let ascii: String = chars[start..end]
        .iter()
        .map(|&c| char::from_digit(digit_value(c).unwrap_or(0), 10).unwrap_or('0'))
        .collect();
    let trimmed = ascii.trim_start_matches('0');
    if trimmed.is_empty() {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

fn glued_before(chars: &[char], start: usize) -> bool {
    start > 0 && is_digit(chars[start - 1])
}

fn currency_word(c: char) -> Option<&'static str> {
    Some(match c {
        '$' => "دلار",
        '€' => "یورو",
        '£' => "پوند",
        '¥' => "ین",
        '₹' => "روپیه",
        '\u{FDFC}' => "ریال",
        _ => return None,
    })
}

const MAGNITUDES: [&str; 4] = ["هزار", "میلیون", "میلیارد", "تریلیون"];

/// A magnitude word at `start`, followed by a non-letter boundary.
fn magnitude_at(chars: &[char], start: usize) -> Option<(&'static str, usize)> {
    for word in MAGNITUDES {
        let word_chars: Vec<char> = word.chars().collect();
        if start + word_chars.len() <= chars.len()
            && chars[start..start + word_chars.len()] == word_chars[..]
        {
            let end = start + word_chars.len();
            if chars.get(end).is_none_or(|c| !c.is_alphabetic()) {
                return Some((word, end));
            }
        }
    }
    None
}

fn skip_spaces(chars: &[char], mut index: usize) -> usize {
    while chars.get(index).is_some_and(|c| c.is_whitespace()) {
        index += 1;
    }
    index
}

/// Spoken degree suffix at `start` (`°C`, `℃`, `°F` or a bare `°`).
fn degree_at(chars: &[char], start: usize) -> Option<(&'static str, usize)> {
    match chars.get(start) {
        Some('℃') => Some((" درجه سانتی‌گراد", start + 1)),
        Some('°') => match chars.get(start + 1) {
            Some('C') => Some((" درجه سانتی‌گراد", start + 2)),
            Some('F') => Some((" درجه فارنهایت", start + 2)),
            _ => Some((" درجه", start + 1)),
        },
        _ => None,
    }
}

/// Try an abbreviation at `start` (whole token; optional spaces after the
/// inner dot; optional final dot).
fn try_abbreviation(chars: &[char], start: usize) -> Option<(String, usize)> {
    if start > 0 && (chars[start - 1].is_alphabetic() || is_digit(chars[start - 1])) {
        return None;
    }
    if chars.get(start + 1) != Some(&'.') {
        return None;
    }
    let mut second = start + 2;
    while chars.get(second).is_some_and(|c| c.is_whitespace()) {
        second += 1;
    }
    let expansion = match (chars[start], chars.get(second)) {
        ('ه', Some('ش')) => "هجری شمسی",
        ('ه', Some('ق')) => "هجری قمری",
        ('ق', Some('م')) => "قبل از میلاد",
        _ => return None,
    };
    let mut end = second + 1;
    if chars.get(end) == Some(&'.') {
        end += 1;
    }
    if chars
        .get(end)
        .is_some_and(|c| c.is_alphabetic() || is_digit(*c))
    {
        return None;
    }
    Some((expansion.to_string(), end))
}

fn collapse_double_spaces(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut previous_space = false;
    for c in text.chars() {
        if c == ' ' {
            if !previous_space {
                result.push(c);
            }
            previous_space = true;
        } else {
            result.push(c);
            previous_space = false;
        }
    }
    result
}

/// Try a date at `start` (`Y/M/D` or `Y-M-D`, same separator twice). Returns
/// the replacement (the original text when the shape matches but the date
/// is out of range) and the index just past the pattern.
fn try_date(chars: &[char], start: usize) -> Option<(String, usize)> {
    let year = digit_run(chars, start)?;
    if year.end - start != 4 {
        return None;
    }
    let separator = *chars.get(year.end)?;
    if separator != '/' && separator != '-' {
        return None;
    }
    let month = digit_run(chars, year.end + 1)?;
    if month.end - (year.end + 1) > 2 || chars.get(month.end) != Some(&separator) {
        return None;
    }
    let day = digit_run(chars, month.end + 1)?;
    if day.end - (month.end + 1) > 2 {
        return None;
    }
    let original: String = chars[start..day.end].iter().collect();
    let (year_value, month_value, day_value) = (year.value, month.value, day.value);
    if !(1..=12).contains(&month_value) || !(1..=31).contains(&day_value) {
        return Some((original, day.end));
    }
    let month_name = match year_value {
        1200..=1499 => JALALI_MONTHS[(month_value - 1) as usize],
        1900..=2100 => GREGORIAN_MONTHS[(month_value - 1) as usize],
        _ => return Some((original, day.end)),
    };
    Some((format!("{day_value} {month_name} {year_value}"), day.end))
}

/// Try a time at `start` (`H:MM` or `H:MM:SS`). Same return shape as
/// [`try_date`].
fn try_time(chars: &[char], start: usize) -> Option<(String, usize)> {
    let hour = digit_run(chars, start)?;
    if hour.end - start > 2 || chars.get(hour.end) != Some(&':') {
        return None;
    }
    let minute = digit_run(chars, hour.end + 1)?;
    if minute.end - (hour.end + 1) != 2 {
        return None;
    }
    let mut end = minute.end;
    let mut second: Option<DigitRun> = None;
    if chars.get(end) == Some(&':') && chars.get(end + 1).is_some_and(|&c| is_digit(c)) {
        let seconds = digit_run(chars, end + 1)?;
        if seconds.end - (end + 1) != 2 {
            return None;
        }
        end = seconds.end;
        second = Some(seconds);
    }
    let original: String = chars[start..end].iter().collect();
    if hour.value > 23 || minute.value > 59 || second.as_ref().is_some_and(|s| s.value > 59) {
        return Some((original, end));
    }
    if minute.value == 0 && second.as_ref().is_none_or(|s| s.value == 0) {
        return Some((format!("{}", hour.value), end));
    }
    let mut spoken = format!("{} و {} دقیقه", hour.value, minute.value);
    if let Some(seconds) = second
        && seconds.value != 0
    {
        spoken.push_str(&format!(" و {} ثانیه", seconds.value));
    }
    Some((spoken, end))
}

/// Try a range at `start` (`A-B`, `-`/`–`/`—`, optional spaces around it,
/// both sides 1–4 digits with no leading zero). A dashed number such as
/// `0912-345-6789` or `978-3-16` is not a range: the first number must
/// not be preceded by `-`, `–`, `—` or `/`, and the second must not be
/// followed by one of those and then a digit.
fn try_range(chars: &[char], start: usize) -> Option<(String, usize)> {
    if start > 0 && matches!(chars[start - 1], '-' | '–' | '—' | '/') {
        return None;
    }
    let first = digit_run(chars, start)?;
    if first.end - start > 4 || first.leading_zero {
        return None;
    }
    let mut dash = first.end;
    while chars.get(dash).is_some_and(|c| c.is_whitespace()) {
        dash += 1;
    }
    if !matches!(chars.get(dash), Some('-' | '–' | '—')) {
        return None;
    }
    let mut second_start = dash + 1;
    while chars.get(second_start).is_some_and(|c| c.is_whitespace()) {
        second_start += 1;
    }
    let second = digit_run(chars, second_start)?;
    if second.end - second_start > 4 || second.leading_zero {
        return None;
    }
    if matches!(chars.get(second.end), Some('-' | '–' | '—' | '/'))
        && chars.get(second.end + 1).is_some_and(|&c| is_digit(c))
    {
        return None;
    }
    Some((format!("{} تا {}", first.value, second.value), second.end))
}

/// Try a Persian decimal at `start` (`A/B`, not followed by another `/`).
fn try_decimal(chars: &[char], start: usize) -> Option<(String, usize)> {
    let first = digit_run(chars, start)?;
    if chars.get(first.end) != Some(&'/') {
        return None;
    }
    let second = digit_run(chars, first.end + 1)?;
    if chars.get(second.end) == Some(&'/') {
        return None;
    }
    // The fraction digits keep their leading zeros: `3/05` is 3.05.
    let fraction: String = chars[first.end + 1..second.end]
        .iter()
        .map(|&c| char::from_digit(digit_value(c).unwrap_or(0), 10).unwrap_or('0'))
        .collect();
    Some((
        format!(
            "{}.{}",
            normalized_digits(chars, start, first.end),
            fraction
        ),
        second.end,
    ))
}

/// Expand dates, times, ranges, signs, Persian decimals, currency,
/// symbols and abbreviations in Persian text. Text that matches no rule
/// is returned unchanged.
#[must_use]
pub fn expand(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut result = String::with_capacity(text.len());
    let mut symbol_changed = false;
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        if let Some((spoken, end)) = try_abbreviation(&chars, index) {
            result.push_str(&spoken);
            index = end;
            continue;
        }
        if let Some(word) = currency_word(c) {
            symbol_changed = true;
            let number_start = skip_spaces(&chars, index + 1);
            if let Some(run) = digit_run(&chars, number_start) {
                let number = normalized_digits(&chars, number_start, run.end);
                // `$۵ میلیون` → `5 میلیون دلار`: the currency word goes
                // after a following magnitude word.
                if chars.get(run.end) == Some(&' ')
                    && let Some((magnitude, magnitude_end)) = magnitude_at(&chars, run.end + 1)
                {
                    result.push_str(&format!("{number} {magnitude} {word}"));
                    index = magnitude_end;
                } else {
                    result.push_str(&format!("{number} {word}"));
                    index = run.end;
                }
            } else {
                result.push_str(word);
                index += 1;
            }
            continue;
        }
        if is_digit(c) && !glued_before(&chars, index) {
            if let Some((spoken, end)) = try_date(&chars, index)
                .or_else(|| try_time(&chars, index))
                .or_else(|| {
                    // A run of more than 4 digits is never a range side.
                    try_range(&chars, index)
                })
                .or_else(|| try_decimal(&chars, index))
            {
                result.push_str(&spoken);
                index = end;
                continue;
            }
            let run = digit_run(&chars, index).expect("digit at index");
            let number = normalized_digits(&chars, index, run.end);
            // A currency symbol right after the number (optional space).
            let symbol_at = skip_spaces(&chars, run.end);
            if let Some(word) = chars.get(symbol_at).and_then(|&s| currency_word(s)) {
                symbol_changed = true;
                let after = skip_spaces(&chars, symbol_at + 1);
                if symbol_at + 1 < after
                    && let Some((magnitude, magnitude_end)) = magnitude_at(&chars, after)
                {
                    result.push_str(&format!("{number} {magnitude} {word}"));
                    index = magnitude_end;
                } else {
                    result.push_str(&format!("{number} {word}"));
                    index = symbol_at + 1;
                }
                continue;
            }
            // A degree sign right after the number (optional space).
            if let Some((suffix, end)) = degree_at(&chars, symbol_at) {
                symbol_changed = true;
                result.push_str(&number);
                result.push_str(suffix);
                index = end;
                continue;
            }
            // A number taking part in an operator expression is rewritten.
            if matches!(chars.get(run.end), Some('+' | '=' | '×' | '÷')) {
                result.push_str(&number);
            } else {
                result.extend(&chars[index..run.end]);
            }
            index = run.end;
            continue;
        }
        if (c == '-' || c == '−')
            && (index == 0 || chars[index - 1].is_whitespace() || chars[index - 1] == '(')
            && chars.get(index + 1).is_some_and(|&next| is_digit(next))
        {
            let run = digit_run(&chars, index + 1).expect("digit after sign");
            result.push_str("منفی ");
            result.push_str(&normalized_digits(&chars, index + 1, run.end));
            index = run.end;
            continue;
        }
        if let Some((suffix, end)) = degree_at(&chars, index) {
            symbol_changed = true;
            result.push_str(suffix);
            index = end;
            continue;
        }
        let operator: Option<(&str, usize)> = match c {
            '×' => Some((" در ", index + 1)),
            '÷' => Some((" تقسیم بر ", index + 1)),
            '=' if chars.get(index + 1).is_some_and(|&n| is_digit(n)) => {
                let run = digit_run(&chars, index + 1).expect("digit after =");
                result.push_str(" مساوی ");
                result.push_str(&normalized_digits(&chars, index + 1, run.end));
                symbol_changed = true;
                index = run.end;
                continue;
            }
            '=' => Some((" مساوی ", index + 1)),
            '+' if chars.get(index + 1).is_some_and(|&n| is_digit(n)) => {
                let run = digit_run(&chars, index + 1).expect("digit after +");
                result.push_str(" به\u{200c}علاوه ");
                result.push_str(&normalized_digits(&chars, index + 1, run.end));
                symbol_changed = true;
                index = run.end;
                continue;
            }
            '#' if chars.get(index + 1).is_some_and(|&n| is_digit(n)) => {
                let run = digit_run(&chars, index + 1).expect("digit after #");
                result.push_str("شماره ");
                result.push_str(&normalized_digits(&chars, index + 1, run.end));
                symbol_changed = true;
                index = run.end;
                continue;
            }
            '@' if index > 0
                && chars[index - 1].is_ascii_alphanumeric()
                && chars
                    .get(index + 1)
                    .is_some_and(|n| n.is_ascii_alphanumeric()) =>
            {
                Some((" ات ", index + 1))
            }
            '.' if index > 0
                && chars[index - 1].is_ascii_alphanumeric()
                && chars
                    .get(index + 1)
                    .is_some_and(|n| n.is_ascii_alphanumeric())
                && (chars[index - 1].is_ascii_alphabetic()
                    || chars[index + 1].is_ascii_alphabetic()) =>
            {
                Some((" دات ", index + 1))
            }
            _ => None,
        };
        if let Some((spoken, end)) = operator {
            symbol_changed = true;
            result.push_str(spoken);
            index = end;
            continue;
        }
        result.push(c);
        index += 1;
    }
    if symbol_changed {
        collapse_double_spaces(&result)
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_times() {
        assert_eq!(expand("ساعت ۸:۳۰ صبح"), "ساعت 8 و 30 دقیقه صبح");
        assert_eq!(expand("۱۰:۰۰"), "10");
        assert_eq!(expand("08:05:09"), "8 و 5 دقیقه و 9 ثانیه");
        assert_eq!(expand("۱۴۰۵/۰۷/۱۴"), "14 مهر 1405");
        assert_eq!(expand("2026-11-13"), "13 نوامبر 2026");
        assert_eq!(expand("۱۴۰۵/۱۳/۰۱"), "۱۴۰۵/۱۳/۰۱");
    }

    #[test]
    fn currency_symbols_and_abbreviations() {
        assert_eq!(expand("$۲۵"), "25 دلار");
        assert_eq!(expand("۲۰€"), "20 یورو");
        assert_eq!(expand("$۵ میلیون"), "5 میلیون دلار");
        assert_eq!(expand("۲۵°C"), "25 درجه سانتی‌گراد");
        assert_eq!(expand("۲+۲=۴"), "2 به\u{200c}علاوه 2 مساوی 4");
        assert_eq!(expand("a@b.com"), "a ات b دات com");
        assert_eq!(expand("3.5"), "3.5");
        assert_eq!(expand("سال ۱۳۵۷ ه.ش"), "سال ۱۳۵۷ هجری شمسی");
        assert_eq!(expand("۵۰۰ ق.م."), "۵۰۰ قبل از میلاد");
        assert_eq!(expand("#۳"), "شماره 3");
    }

    #[test]
    fn ranges_signs_and_decimals() {
        assert_eq!(expand("0912-345-6789"), "0912-345-6789");
        assert_eq!(expand("978-3-16"), "978-3-16");
        assert_eq!(expand("۱۴۰۴–۱۴۰۵"), "1404 تا 1405");
        assert_eq!(expand("۵-۱۰ نفر"), "5 تا 10 نفر");
        assert_eq!(expand("021-88776655"), "021-88776655");
        assert_eq!(expand("دما -۵ درجه"), "دما منفی 5 درجه");
        assert_eq!(expand("۳/۵ درصد"), "3.5 درصد");
        assert_eq!(expand("متن ساده بدون عدد."), "متن ساده بدون عدد.");
    }
}
