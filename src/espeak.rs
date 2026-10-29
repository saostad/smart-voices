//! Serialized access to the single statically linked espeak-ng instance.
use espeak_rs_sys as es;
use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    path::{Path, PathBuf},
    sync::Mutex,
};
static STATE: Mutex<Option<PathBuf>> = Mutex::new(None);
// espeak-ng 1.52 provides the clause translator internally, but its public
// TextToPhonemes wrapper discards the terminator needed by Piper. This uses
// the same decoder/translator sequence and preserves the clause terminator.
unsafe extern "C" {
    static mut p_decoder: *mut c_void;
    static mut translator: *mut c_void;
    fn create_text_decoder() -> *mut c_void;
    fn text_decoder_decode_string_multibyte(
        decoder: *mut c_void,
        text: *const c_void,
        encoding: c_int,
        mode: c_int,
    ) -> c_int;
    fn text_decoder_get_buffer(decoder: *mut c_void) -> *const c_void;
    fn TranslateClauseWithTerminator(
        tr: *mut c_void,
        tone: *mut c_int,
        voice: *mut *mut c_char,
        terminator: *mut c_int,
    );
    fn GetTranslatedPhonemeString(mode: c_int) -> *const c_char;
}
fn clauses(data: &Path, text: &str, voice: &str, tie: bool) -> Result<Vec<(String, i32)>, String> {
    let mut state = STATE.lock().map_err(|_| "pronunciation lock failed")?;
    if state.as_deref() != Some(data) {
        let path = CString::new(data.to_string_lossy().as_bytes())
            .map_err(|_| "invalid pronunciation data path")?;
        if state.is_some() {
            unsafe {
                es::espeak_Terminate();
            }
        }
        let rate = unsafe {
            es::espeak_Initialize(
                es::espeak_AUDIO_OUTPUT_AUDIO_OUTPUT_SYNCHRONOUS,
                0,
                path.as_ptr(),
                es::espeakINITIALIZE_DONT_EXIT as i32,
            )
        };
        if rate <= 0 {
            *state = None;
            return Err("could not initialize pronunciation data".into());
        }
        *state = Some(data.into());
    }
    let voice = CString::new(voice).map_err(|_| "invalid pronunciation voice")?;
    // The tarball names the British voice file `en`; en-gb is its
    // language tag, so select by language rather than by file identifier.
    let status = if voice.as_bytes() == b"en-gb" {
        let mut properties = es::espeak_VOICE {
            name: std::ptr::null(),
            languages: voice.as_ptr(),
            identifier: std::ptr::null(),
            gender: 0,
            age: 0,
            variant: 0,
            xx1: 0,
            score: 0,
            spare: std::ptr::null_mut(),
        };
        unsafe { es::espeak_SetVoiceByProperties(&mut properties) }
    } else {
        unsafe { es::espeak_SetVoiceByName(voice.as_ptr()) }
    };
    if status != es::espeak_ERROR_EE_OK {
        return Err("could not select pronunciation voice".into());
    }
    let text = CString::new(text.replace('\0', "")).map_err(|_| "invalid pronunciation text")?;
    let mut ptr = text.as_ptr().cast::<c_void>();
    let mut out = Vec::new();
    let mode = if tie {
        (es::espeakINITIALIZE_PHONEME_IPA | es::espeakPHONEMES_TIE | (('^' as u32) << 8)) as i32
    } else {
        2
    };
    while !ptr.is_null() {
        let mut term = 0;
        let result = unsafe {
            if p_decoder.is_null() {
                p_decoder = create_text_decoder();
            }
            if p_decoder.is_null() {
                return Err("could not allocate pronunciation decoder".into());
            }
            if text_decoder_decode_string_multibyte(p_decoder, ptr, 1, es::espeakCHARS_UTF8 as i32)
                != 0
            {
                return Err("could not decode pronunciation text".into());
            }
            TranslateClauseWithTerminator(
                translator,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut term,
            );
            ptr = text_decoder_get_buffer(p_decoder);
            GetTranslatedPhonemeString(mode)
        };
        if result.is_null() {
            return Err("pronunciation returned no phonemes".into());
        }
        let s = unsafe { CStr::from_ptr(result) }
            .to_str()
            .map_err(|_| "pronunciation returned invalid UTF-8")?
            .to_owned();
        out.push((s, term));
    }
    Ok(out)
}
pub(crate) fn english(data: &Path, text: &str, british: bool) -> Result<String, String> {
    let raw = clauses(data, text, if british { "en-gb" } else { "en-us" }, true)?
        .into_iter()
        .map(|(s, _)| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if raw.is_empty() {
        return Err("English pronunciation produced no phonemes".into());
    }
    Ok(to_misaki(&raw, british))
}
pub(crate) fn piper(data: &Path, text: &str, voice: &str) -> Result<Vec<String>, String> {
    use unicode_normalization::UnicodeNormalization;
    let mut sentences = Vec::new();
    let mut sentence = String::new();
    for (ps, term) in clauses(data, text, voice, false)? {
        let mut flag = false;
        for c in ps.nfd() {
            if c == '(' {
                flag = true;
            } else if c == ')' {
                flag = false;
            } else if !flag {
                sentence.push(c);
            }
        }
        match term & 0xfffff {
            0x80028 => sentence.push('.'),
            0x82028 => sentence.push('?'),
            0x8302d => sentence.push('!'),
            0x41014 => sentence.push_str(", "),
            0x4001e => sentence.push_str(": "),
            0x4101e => sentence.push_str("; "),
            _ => {}
        }
        if term & 0x80000 != 0 && !sentence.is_empty() {
            sentences.push(std::mem::take(&mut sentence));
        }
    }
    if !sentence.is_empty() {
        sentences.push(sentence);
    }
    Ok(sentences)
}
const E2M: &[(&str, &str)] = &[
    ("ʔˌn\u{329}", "ʔn"),
    ("ʔn\u{329}", "ʔn"),
    ("a^ɪ", "I"),
    ("a^ʊ", "W"),
    ("d^ʒ", "ʤ"),
    ("e^ɪ", "A"),
    ("t^ʃ", "ʧ"),
    ("ɔ^ɪ", "Y"),
    ("ə^l", "ᵊl"),
    ("ʲo", "jo"),
    ("ʲə", "jə"),
    ("e", "A"),
    ("ʲ", ""),
    ("ɚ", "əɹ"),
    ("r", "ɹ"),
    ("x", "k"),
    ("ç", "k"),
    ("ɐ", "ə"),
    ("ɬ", "l"),
    ("\u{303}", ""),
];

fn to_misaki(raw: &str, british: bool) -> String {
    let mut ps = raw.to_string();
    for (old, new) in E2M {
        ps = ps.replace(old, new);
    }
    let mut chars: Vec<char> = Vec::new();
    for c in ps.chars() {
        if c == '\u{329}' {
            if let Some(last) = chars.pop() {
                if !last.is_whitespace() {
                    chars.push('ᵊ');
                }
                chars.push(last);
            }
        } else {
            chars.push(c);
        }
    }
    ps = chars.into_iter().collect();
    if british {
        ps = ps
            .replace("e^ə", "ɛː")
            .replace("iə", "ɪə")
            .replace("ə^ʊ", "Q");
    } else {
        ps = ps
            .replace("o^ʊ", "O")
            .replace("ɜːɹ", "ɜɹ")
            .replace("ɜː", "ɜɹ")
            .replace("ɪə", "iə")
            .replace('ː', "");
    }
    ps = ps.replace('o', "ɔ");
    ps = ps.replace('ɾ', "T").replace('ʔ', "t");
    ps.replace('^', "")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_e2m_conversions() {
        assert_eq!(
            to_misaki("a^ɪ a^ʊ d^ʒ e^ɪ t^ʃ ɔ^ɪ ə^l", false),
            "I W ʤ A ʧ Y ᵊl"
        );
        assert_eq!(to_misaki("ʔˌn\u{329} ɚ ɾ ɜːɹ o^ʊ", false), "tn əɹ T ɜɹ O");
        assert_eq!(to_misaki("ə^ʊ iə", true), "Q ɪə");
    }
}
