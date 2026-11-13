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
/// both sides 1–4 digits with no leading zero).
fn try_range(chars: &[char], start: usize) -> Option<(String, usize)> {
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

/// Expand dates, times, ranges, signs and Persian decimals in Persian
/// text. Text that matches no rule is returned unchanged.
#[must_use]
pub fn expand(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut result = String::with_capacity(text.len());
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
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
            // No pattern: copy the whole digit run as it is.
            let run = digit_run(&chars, index).expect("digit at index");
            result.extend(&chars[index..run.end]);
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
        result.push(c);
        index += 1;
    }
    result
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
    fn ranges_signs_and_decimals() {
        assert_eq!(expand("۵-۱۰ نفر"), "5 تا 10 نفر");
        assert_eq!(expand("021-88776655"), "021-88776655");
        assert_eq!(expand("دما -۵ درجه"), "دما منفی 5 درجه");
        assert_eq!(expand("۳/۵ درصد"), "3.5 درصد");
        assert_eq!(expand("متن ساده بدون عدد."), "متن ساده بدون عدد.");
    }
}
