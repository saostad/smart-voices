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
/// espeak-ng keeps `<data>/espeak-ng-data` in a fixed buffer
/// (`N_PATH_HOME`, 160 bytes, 230 on Windows), and init fails beyond it.
#[cfg(windows)]
const MAX_DATA_PATH_BYTES: usize = 229 - "/espeak-ng-data".len();
#[cfg(not(windows))]
const MAX_DATA_PATH_BYTES: usize = 159 - "/espeak-ng-data".len();

fn link_root() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("smart-voices")
        .join("espeak-links")
}

/// A path to `data` that fits espeak-ng's buffer: `data` itself, or else a
/// symlink to it under `link_root`.
fn short_data_path(data: &Path, link_root: &Path) -> Result<PathBuf, String> {
    let fits = |path: &Path| path.as_os_str().len() <= MAX_DATA_PATH_BYTES;
    if fits(data) {
        return Ok(data.to_path_buf());
    }
    let too_long = || {
        format!(
            "The voice data folder's path is too long ({} characters, limit {MAX_DATA_PATH_BYTES}). Set SMART_VOICES_MODELS to a shorter folder.",
            data.as_os_str().len()
        )
    };
    let digest = ring::digest::digest(&ring::digest::SHA256, data.as_os_str().as_encoded_bytes());
    let name: String = digest.as_ref()[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let link = link_root.join(name);
    if !fits(&link) {
        return Err(too_long());
    }
    if std::fs::read_link(&link).is_ok_and(|target| target == data) {
        return Ok(link);
    }
    std::fs::create_dir_all(link_root).map_err(|_| too_long())?;
    let _ = std::fs::remove_file(&link);
    #[cfg(unix)]
    let created = std::os::unix::fs::symlink(data, &link);
    #[cfg(windows)]
    let created = std::os::windows::fs::symlink_dir(data, &link);
    created.map_err(|_| too_long())?;
    Ok(link)
}

fn clauses(data: &Path, text: &str, voice: &str, tie: bool) -> Result<Vec<(String, i32)>, String> {
    let mut state = STATE.lock().map_err(|_| "pronunciation lock failed")?;
    if state.as_deref() != Some(data) {
        let usable = short_data_path(data, &link_root())?;
        let path = CString::new(usable.to_string_lossy().as_bytes())
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

#[cfg(all(test, unix))]
mod path_tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("smart-voices-espeak-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn short_paths_are_used_as_they_are() {
        let path = Path::new("/models/kokoro");
        assert_eq!(short_data_path(path, Path::new("/unused")).unwrap(), path);
    }

    #[test]
    fn long_paths_get_a_short_link_to_the_same_folder() {
        let root = temp_dir("long");
        let data = root.join("d".repeat(100)).join("e".repeat(100));
        std::fs::create_dir_all(data.join("espeak-ng-data")).unwrap();
        let links = root.join("l");
        let link = short_data_path(&data, &links).unwrap();
        assert!(link.as_os_str().len() <= MAX_DATA_PATH_BYTES);
        assert!(link.join("espeak-ng-data").is_dir());
        // A second call reuses it, and a stale link is repointed.
        assert_eq!(short_data_path(&data, &links).unwrap(), link);
        let other = root.join("d".repeat(100)).join("f".repeat(100));
        std::fs::create_dir_all(&other).unwrap();
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&other, &link).unwrap();
        assert_eq!(short_data_path(&data, &links).unwrap(), link);
        assert_eq!(std::fs::read_link(&link).unwrap(), data);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn long_path_with_a_long_link_root_names_the_setting_to_change() {
        let data = PathBuf::from(format!("/{}", "d".repeat(200)));
        let error = short_data_path(&data, &data).unwrap_err();
        assert!(error.contains("SMART_VOICES_MODELS"), "{error}");
    }
}
