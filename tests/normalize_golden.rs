//! The Rust normalizer must match the Python original it was ported from
//! (`underdog_woof/normalize.py`, NeMo not installed) on every fixture line:
//! the 500 English sentences of `gradium/tts-hard-sentences-202610` (CC BY 4.0)
//! plus hand-written cases for each rule family. Three fixture lines are
//! corrected by hand: the Python reads "1:25,000" as "one twenty five,zero", and
//! "Bus no. 42" as "Bus number forty two", which Phonon misreads.

use smart_voices::normalize::{DateOrder, NormalizeOptions, normalize};

/// True for a slashed date with a year whose first two parts are both <= 12.
fn has_ambiguous_full_date(text: &str) -> bool {
    text.split(|c: char| !c.is_ascii_digit() && c != '/')
        .any(|token| {
            let parts: Vec<&str> = token.split('/').collect();
            let small = |p: &str| p.parse::<u32>().is_ok_and(|n| (1..=12).contains(&n));
            parts.len() == 3 && small(parts[0]) && small(parts[1]) && parts[2].len() == 4
        })
}

/// Text without hyphens or case, for comparing joined syllables.
fn comparable(text: &str) -> String {
    text.to_lowercase().replace('-', "")
}

#[test]
fn matches_python_output() {
    let path = std::env::var("NORMALIZE_GOLDEN")
        .unwrap_or_else(|_| "tests/fixtures/normalize_golden.json".to_string());
    let raw = std::fs::read_to_string(&path).expect("golden fixture");
    let cases: Vec<serde_json::Value> = serde_json::from_str(&raw).expect("golden JSON");
    assert!(
        cases.len() >= 500,
        "fixture looks truncated: {}",
        cases.len()
    );
    // Day first is what the Python does for short dates; the differences
    // this app chose on purpose are in `normalize_overrides.json`.
    let plain = NormalizeOptions {
        normalize_text: true,
        respell_names: false,
        date_order: DateOrder::DayFirst,
    };
    let names = NormalizeOptions {
        normalize_text: true,
        respell_names: true,
        date_order: DateOrder::DayFirst,
    };
    let mut failures = Vec::new();
    let mut skipped = 0;
    for case in &cases {
        let input = case["in"].as_str().expect("input");
        // Intentional change from the Python, tested in `normalize.rs`: it reads
        // 09/10/2026 month first and 12/10 day first in the same sentence.
        if has_ambiguous_full_date(input) {
            skipped += 1;
            continue;
        }
        for (options, key) in [(plain, "plain"), (names, "names")] {
            let got = normalize(input, options);
            let want = case[key].as_str().expect("expected");
            // This app joins the lexicon's syllables (Bah-gahn → Bahgahn).
            let same = if key == "names" {
                comparable(&got) == comparable(want)
            } else {
                got == want
            };
            if !same {
                failures.push(format!("[{key}] {input}\n  want: {want}\n  got:  {got}"));
            }
        }
    }
    assert!(skipped <= 30, "skipped too many: {skipped}");
    assert!(
        failures.is_empty(),
        "{} of {} differ:\n{}",
        failures.len(),
        cases.len() * 2,
        failures[..failures.len().min(40)].join("\n")
    );
}
