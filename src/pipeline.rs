//! Speak pipeline: sanitize → chunk.
//!
//! Chunks keep their display text; pronunciation rules apply to each chunk's
//! text only when it is synthesized, as Swift does. The `pronunciations`
//! argument names the dictionary the caller must apply per chunk at synthesis
//! time (the next round's playback controller will own that step).

use std::sync::atomic::{AtomicBool, Ordering};

use crate::chunk::{SpeechChunk, SpeechChunker};
use crate::lang::Lang;
use crate::latin_fa::{Persianized, persianize};
use crate::normalize::{NormalizeOptions, normalize};
use crate::pronounce::Pronunciations;
use crate::sanitize::{SanitizerOptions, sanitize};

/// Sanitize `text` and split it into speakable chunks (display text kept).
#[must_use]
pub fn prepare(
    text: &str,
    options: &SanitizerOptions,
    _pronunciations: &Pronunciations,
) -> Vec<SpeechChunk> {
    let segments = sanitize(text, options);
    SpeechChunker::new().chunks(&segments)
}

/// Tell the user how a Persian reading handled Latin words: log every
/// time (never the words themselves, which are the user's text), and
/// show each desktop notice once per process, each with its own flag.
fn report_persian_reading(result: &Persianized, notify: &dyn Fn(&str, &str)) {
    if !result.approximated.is_empty() {
        eprintln!(
            "warn: fallback: true, attempted=english_word_list, reason=word_not_listed, taken=approximate_persian_spelling, count={}",
            result.approximated.len()
        );
        static APPROX_NOTICE_SHOWN: AtomicBool = AtomicBool::new(false);
        if !APPROX_NOTICE_SHOWN.swap(true, Ordering::SeqCst) {
            notify(
                "Persian voice",
                "Some English words were read with an approximate pronunciation. To fix a word, open Edit Pronunciations\u{2026} and add it with its Persian spelling.",
            );
        }
    }
    if result.skipped {
        eprintln!(
            "warn: fallback: true, attempted=persian_voice, reason=unsupported_script, taken=skipped_text"
        );
        static SKIPPED_NOTICE_SHOWN: AtomicBool = AtomicBool::new(false);
        if !SKIPPED_NOTICE_SHOWN.swap(true, Ordering::SeqCst) {
            notify(
                "Persian voice",
                "Some text was skipped: the Persian voice can only read Persian and English letters.",
            );
        }
    }
}

/// The text a voice reads for a chunk: the user's pronunciation rules first,
/// so they win, then the English normalizer. Persian chunks get only the
/// user's Persian pronunciation rules, then Latin words are converted to
/// Persian spelling so the Persian engine reads them instead of dropping
/// them.
#[must_use]
pub fn spoken_text(
    chunk_text: &str,
    language: Lang,
    pronunciations: &Pronunciations,
    options: NormalizeOptions,
) -> String {
    match language {
        Lang::English => normalize(&pronunciations.apply(chunk_text), options),
        Lang::Persian => {
            let result = persianize(&pronunciations.apply_persian(chunk_text));
            report_persian_reading(&result, &crate::notification::show);
            result.text
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sanitize::{BareUrls, CodeBlocks, Tables};

    #[test]
    fn prepare_keeps_display_text() {
        let pronunciations = Pronunciations::with_defaults();
        let chunks = prepare("I love SQL.", &SanitizerOptions::default(), &pronunciations);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, "I love SQL.");
        // ... but synthesis-time application speaks the rule:
        assert_eq!(pronunciations.apply(&chunks[0].text), "I love sequel.");
    }

    #[test]
    fn spoken_text_normalizes_english_only_and_rules_win() {
        let rules = Pronunciations::new(vec![crate::pronounce::PronunciationEntry::new(
            "#1", "the best",
        )]);
        let options = NormalizeOptions::default();
        assert_eq!(
            spoken_text("We are #1 at 5 kg.", Lang::English, &rules, options),
            "We are the best at five kilograms."
        );
        assert_eq!(
            spoken_text("سلام 5 kg.", Lang::Persian, &rules, options),
            "سلام 5 کیلوگرم."
        );
    }

    #[test]
    fn spoken_text_persian_rules_win_and_latin_words_are_read() {
        let options = NormalizeOptions::default();
        let rules = Pronunciations::new(vec![crate::pronounce::PronunciationEntry::new(
            "GitHub",
            "گیت هاب",
        )]);
        assert_eq!(
            spoken_text("از GitHub استفاده کن", Lang::Persian, &rules, options),
            "از گیت هاب استفاده کن"
        );
        assert_eq!(
            spoken_text(
                "از API استفاده کن",
                Lang::Persian,
                &Pronunciations::with_defaults(),
                options
            ),
            "از ای‌پی‌آی استفاده کن"
        );
    }

    #[test]
    fn report_persian_reading_notifies_once_per_kind() {
        use std::cell::RefCell;
        let notices: RefCell<Vec<(String, String)>> = RefCell::new(Vec::new());
        let notify = |title: &str, body: &str| {
            notices
                .borrow_mut()
                .push((title.to_string(), body.to_string()));
        };
        let approximated = Persianized {
            text: String::new(),
            approximated: vec!["cluster".to_string()],
            skipped: false,
        };
        report_persian_reading(&approximated, &notify);
        report_persian_reading(&approximated, &notify);
        let skipped = Persianized {
            text: String::new(),
            approximated: Vec::new(),
            skipped: true,
        };
        report_persian_reading(&skipped, &notify);
        let notices = notices.borrow();
        assert_eq!(notices.len(), 2, "{notices:?}");
        assert_eq!(
            notices[0],
            (
                "Persian voice".to_string(),
                "Some English words were read with an approximate pronunciation. To fix a word, open Edit Pronunciations\u{2026} and add it with its Persian spelling."
                    .to_string()
            )
        );
        assert_eq!(
            notices[1],
            (
                "Persian voice".to_string(),
                "Some text was skipped: the Persian voice can only read Persian and English letters."
                    .to_string()
            )
        );
    }

    #[test]
    fn mixed_unicode_prepare_does_not_panic() {
        let md = "Emoji 🎉👋 test سلام دنیا 中文テスト\n\
            Combining: e\u{301} a\u{308} o\u{303} and ZWJ \u{200d} done.\n\
            \n\
            | Héllo | 世界 |\n\
            | --- | --- |\n\
            | één [1] | ~42 |\n\
            \n\
            Unbalanced **bold and `code and ~~strike plus *star* run___\n\
            \n\
            - Item with `tick and **star\n\
            - [x] Toोड़ो test\n\
            \n\
            Visit https://example.com/üser?q=🎉 for <b>bold</b> &amp; more.\n";
        let pronunciations = Pronunciations::with_defaults();
        let chunks = prepare(md, &SanitizerOptions::default(), &pronunciations);
        assert!(!chunks.is_empty());
        for chunk in &chunks {
            assert!(!chunk.text.is_empty());
        }
        // Every option combination must survive it too.
        for code_blocks in [CodeBlocks::Announce, CodeBlocks::Skip, CodeBlocks::ReadFull] {
            for tables in [Tables::ReadRows, Tables::Announce, Tables::Skip] {
                for bare_urls in [BareUrls::DomainOnly, BareUrls::Skip] {
                    let options = SanitizerOptions {
                        code_blocks,
                        tables,
                        bare_urls,
                        max_table_rows_to_read: 1,
                    };
                    let _ = prepare(md, &options, &pronunciations);
                }
            }
        }
    }
}
