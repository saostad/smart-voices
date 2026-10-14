//! Speak pipeline: sanitize → chunk.
//!
//! Chunks keep their display text; pronunciation rules apply to each chunk's
//! text only when it is synthesized, as Swift does. The `pronunciations`
//! argument names the dictionary the caller must apply per chunk at synthesis
//! time (the next round's playback controller will own that step).

use crate::chunk::{SpeechChunk, SpeechChunker};
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
