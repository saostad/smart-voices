//! Native Kokoro and Piper ONNX inference; model metadata owns geometry.
use crate::engine::{ModelVariant, PersianVoice};
use ort::{session::Session, value::Tensor};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn session(path: &Path) -> Result<Session, String> {
    Session::builder(crate::pocket::runtime::ort_environment().map_err(err)?)
        .map_err(err)?
        .with_intra_threads(crate::pocket::runtime::thread_count())
        .map_err(err)?
        .commit_from_file(path)
        .map_err(err)
}
fn meta(s: &Session, key: &str) -> Result<String, String> {
    s.metadata()
        .map_err(err)?
        .custom(key)
        .ok_or_else(|| format!("missing model metadata: {key}"))
}
fn number<T: std::str::FromStr>(s: &Session, key: &str) -> Result<T, String> {
    meta(s, key)?
        .parse()
        .map_err(|_| format!("invalid model metadata: {key}"))
}
fn tensor<T: ort::value::PrimitiveTensorElementType + std::fmt::Debug + Clone + 'static>(
    shape: Vec<usize>,
    data: Vec<T>,
) -> Result<Tensor<T>, String> {
    Tensor::from_array((shape, data.into_boxed_slice())).map_err(err)
}
pub(crate) fn tokens(text: &str) -> Result<HashMap<char, i64>, String> {
    let mut out = HashMap::new();
    for line in text.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        let (sym, id) = match parts.as_slice() {
            [id] => (" ", *id),
            [sym, id] => (*sym, *id),
            _ => return Err("invalid tokens.txt line".into()),
        };
        let mut chars = sym.chars();
        let c = chars.next().ok_or("empty token")?;
        if chars.next().is_some() {
            return Err("token is not a codepoint".into());
        }
        let id = id.parse().map_err(|_| "invalid token id")?;
        if out.insert(c, id).is_some() {
            return Err("duplicate token".into());
        }
    }
    Ok(out)
}
fn load_tokens(dir: &Path) -> Result<HashMap<char, i64>, String> {
    tokens(&std::fs::read_to_string(dir.join("tokens.txt")).map_err(err)?)
}
pub(crate) fn scale_silence(
    samples: &[f32],
    sr: i32,
    scale: f32,
    min: f32,
    amplitude: f32,
) -> Vec<f32> {
    if scale == 1. {
        return samples.to_vec();
    }
    let threshold = (sr as f32 * min) as usize;
    let mut out = Vec::new();
    let mut i = 0;
    while i < samples.len() {
        if samples[i].abs() > amplitude {
            out.push(samples[i]);
            i += 1;
            continue;
        }
        let start = i;
        while i < samples.len() && samples[i].abs() <= amplitude {
            i += 1;
        }
        let len = i - start;
        let qualifies = if i == samples.len() {
            len > threshold
        } else {
            len >= threshold
        };
        if qualifies {
            let n = (len as f32 * scale) as usize;
            out.extend_from_slice(&samples[start..start + n.min(len)]);
            if n > len {
                out.resize(out.len() + n - len, 0.);
            }
        } else {
            out.extend_from_slice(&samples[start..i]);
        }
    }
    out
}
/// Pause settings (scale, minimum seconds, amplitude), re-read once per
/// synthesis so a Pause Length change applies to the next chunk. A missing
/// or invalid settings file falls back to the defaults `Settings::load`
/// already returns (the app warned about the file at startup); it must not
/// fail the reading.
fn pause_values_from(path: Option<&Path>) -> (f32, f32, f32) {
    let (settings, error) = match path {
        Some(path) => crate::settings::Settings::load(path),
        None => {
            eprintln!(
                "warn: fallback: true, attempted=settings_file, reason=settings_directory_unavailable, taken=default_pause_settings"
            );
            let s = crate::settings::Settings::default();
            return (s.silence_scale, s.pause_min_seconds, s.pause_amplitude);
        }
    };
    if let Some(error) = error {
        eprintln!(
            "warn: fallback: true, attempted=settings_file, reason={error}, taken=default_pause_settings"
        );
    }
    (
        settings.silence_scale,
        settings.pause_min_seconds,
        settings.pause_amplitude,
    )
}
fn pause_values() -> (f32, f32, f32) {
    pause_values_from(crate::settings::settings_path().as_deref())
}
/// The espeak fallback notice is shown once per process: unknown words and
/// numbers are a normal part of reading, so a popup per chunk would spam
/// the desktop. The per-chunk warning is still logged every time.
fn take_fallback_notice(shown: &std::sync::atomic::AtomicBool) -> bool {
    !shown.swap(true, std::sync::atomic::Ordering::SeqCst)
}
fn style_offset(
    sid: usize,
    len: usize,
    rows: usize,
    width: usize,
    speakers: usize,
) -> Result<usize, String> {
    if sid >= speakers || len >= rows {
        return Err("invalid Kokoro style selection".into());
    }
    sid.checked_mul(rows)
        .and_then(|v| v.checked_add(len))
        .and_then(|v| v.checked_mul(width))
        .ok_or("Kokoro style dimensions overflow".into())
}
fn kokoro_pieces(
    ps: &str,
    vocab: &HashMap<char, i64>,
    max: usize,
) -> Result<Vec<Vec<i64>>, String> {
    if max == 0 {
        return Err("invalid Kokoro token limit".into());
    }
    Ok(ps
        .split_inclusive(['.', '!', '?', ';'])
        .flat_map(|p| {
            let ids: Vec<_> = p
                .trim()
                .chars()
                .filter_map(|c| vocab.get(&c).copied())
                .collect();
            ids.chunks(max).map(<[i64]>::to_vec).collect::<Vec<_>>()
        })
        .collect())
}
pub(crate) struct Kokoro {
    session: Session,
    root: PathBuf,
    data: PathBuf,
    vocab: HashMap<char, i64>,
    styles: Vec<f32>,
    rows: usize,
    width: usize,
    speakers: usize,
    pub sample_rate: i32,
    lexicon: Option<(bool, crate::misaki::G2P)>,
}
impl Kokoro {
    pub fn load(root: &Path, variant: ModelVariant) -> Result<Self, String> {
        let dir = root.join(variant.dir_name());
        let session = session(&dir.join(variant.model_file()))?;
        let dims = meta(&session, "style_dim")?
            .split(',')
            .map(|s| s.trim().parse::<usize>().map_err(err))
            .collect::<Result<Vec<_>, _>>()?;
        if dims.len() != 3 || dims[1] != 1 || dims[0] < 3 || dims[2] == 0 {
            return Err("invalid style dimensions".into());
        }
        let speakers: usize = number(&session, "n_speakers")?;
        let sample_rate: i32 = number(&session, "sample_rate")?;
        if sample_rate <= 0 {
            return Err("invalid model sample rate".into());
        }
        let bytes = std::fs::read(dir.join("voices.bin")).map_err(err)?;
        let styles: Vec<f32> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect();
        if bytes.len() % 4 != 0
            || Some(styles.len())
                != speakers
                    .checked_mul(dims[0])
                    .and_then(|v| v.checked_mul(dims[2]))
        {
            return Err("invalid voices.bin dimensions".into());
        }
        Ok(Self {
            session,
            root: root.join("misaki-en"),
            data: dir.clone(),
            vocab: load_tokens(&dir)?,
            styles,
            rows: dims[0],
            width: dims[2],
            speakers,
            sample_rate,
            lexicon: None,
        })
    }
    pub fn synthesize(
        &mut self,
        text: &str,
        sid: usize,
        british: bool,
        speed: f32,
    ) -> Result<Vec<f32>, String> {
        if sid >= self.speakers {
            return Err("invalid speaker id".into());
        }
        if self.lexicon.as_ref().map(|p| p.0) != Some(british) {
            self.lexicon = Some((
                british,
                crate::misaki::G2P::load(&self.root, &self.data, british)?,
            ));
        }
        let (ps, used_espeak) = self
            .lexicon
            .as_ref()
            .ok_or("pronunciation data unavailable")?
            .1
            .g2p_with_notice(text)?;
        if used_espeak {
            eprintln!(
                "warn: fallback: true, attempted=misaki_dictionary, reason=unknown_or_numeric_word, taken=espeak_ng, british={british}"
            );
            static NOTICE_SHOWN: std::sync::atomic::AtomicBool =
                std::sync::atomic::AtomicBool::new(false);
            if take_fallback_notice(&NOTICE_SHOWN) {
                crate::notification::show(
                    "English pronunciation",
                    "espeak-ng pronunciation was used for numbers or words outside the English dictionary.",
                );
            }
        }
        let (scale, min, amplitude) = pause_values();
        let mut audio = Vec::new();
        for ids in kokoro_pieces(&ps, &self.vocab, self.rows - 2)? {
            let offset = style_offset(sid, ids.len(), self.rows, self.width, self.speakers)?;
            let style = self
                .styles
                .get(offset..offset + self.width)
                .ok_or("invalid style row")?
                .to_vec();
            let mut padded = vec![0];
            padded.extend_from_slice(&ids);
            padded.push(0);
            let result=self.session.run(ort::inputs!["tokens"=>tensor(vec![1,padded.len()],padded)?,"style"=>tensor(vec![1,self.width],style)?,"speed"=>tensor(vec![1],vec![speed])?]).map_err(err)?;
            let value = result
                .iter()
                .next()
                .ok_or("voice model returned no outputs")?
                .1;
            let (_, data) = value.try_extract_tensor::<f32>().map_err(err)?;
            audio.extend(scale_silence(data, self.sample_rate, scale, min, amplitude));
        }
        if audio.is_empty() {
            return Err("voice produced no audio".into());
        }
        Ok(audio)
    }
}
pub(crate) struct Piper {
    session: Session,
    data: PathBuf,
    vocab: HashMap<char, i64>,
    voice: String,
    scales: [f32; 3],
    pub sample_rate: i32,
}
impl Piper {
    pub fn load(root: &Path, voice: PersianVoice) -> Result<Self, String> {
        let dir = root.join(voice.dir_name());
        let model_file = voice.model_file();
        let session = session(&dir.join(model_file))?;
        let sample_rate: i32 = number(&session, "sample_rate")?;
        if sample_rate <= 0 {
            return Err("invalid model sample rate".into());
        }
        let voice = meta(&session, "voice")?;
        #[derive(serde::Deserialize)]
        struct Config {
            inference: Inference,
        }
        #[derive(serde::Deserialize)]
        struct Inference {
            noise_scale: f32,
            length_scale: f32,
            noise_w: f32,
        }
        let config_path = dir.join(format!("{}.json", model_file));
        let config: Config = serde_json::from_reader(std::io::BufReader::new(
            std::fs::File::open(&config_path)
                .map_err(|e| format!("{}: {e}", config_path.display()))?,
        ))
        .map_err(err)?;
        let p = config.inference;
        let scales = [p.noise_scale, p.length_scale, p.noise_w];
        if scales.iter().any(|v| !v.is_finite() || *v < 0.) || p.length_scale == 0. {
            return Err("invalid Piper inference settings".into());
        }
        let vocab = load_tokens(&dir)?;
        for c in ['^', '_', '$'] {
            if !vocab.contains_key(&c) {
                return Err("missing Piper framing token".into());
            }
        }
        Ok(Self {
            session,
            data: dir,
            vocab,
            voice,
            scales,
            sample_rate,
        })
    }
    pub fn synthesize(&mut self, text: &str, speed: f32) -> Result<Vec<f32>, String> {
        // Like sherpa with its default max_num_sentences=0: all sentences
        // of the chunk run through the model in one call (framing kept in
        // the concatenated sequence), then pauses are scaled once over
        // the whole result, instead of resetting prosody per sentence.
        let sentences = crate::espeak::piper(&self.data, text, &self.voice)?;
        let ids = piper_ids_all(&sentences, &self.vocab)?;
        if ids.is_empty() {
            return Err("voice produced no audio".into());
        }
        let (scale, min, amplitude) = pause_values();
        let len = ids.len();
        let mut inputs = ort::inputs!["input"=>tensor(vec![1,len],ids)?,"input_lengths"=>tensor(vec![1],vec![len as i64])?,"scales"=>tensor(vec![3],vec![self.scales[0],self.scales[1]/speed,self.scales[2]])?];
        if self.session.inputs().iter().any(|i| i.name() == "sid") {
            inputs.push(("sid".into(), tensor(vec![1], vec![0i64])?.into()));
        }
        let result = self.session.run(inputs).map_err(err)?;
        let value = result
            .iter()
            .next()
            .ok_or("voice model returned no outputs")?
            .1;
        let (_, data) = value.try_extract_tensor::<f32>().map_err(err)?;
        if data.is_empty() {
            return Err("voice produced no audio".into());
        }
        Ok(scale_silence(data, self.sample_rate, scale, min, amplitude))
    }
}
fn piper_ids_all(sentences: &[String], vocab: &HashMap<char, i64>) -> Result<Vec<i64>, String> {
    let mut ids = Vec::new();
    for ps in sentences {
        ids.extend(piper_ids(ps, vocab)?);
    }
    Ok(ids)
}
fn piper_ids(ps: &str, vocab: &HashMap<char, i64>) -> Result<Vec<i64>, String> {
    let get = |c| vocab.get(&c).copied().ok_or("missing Piper framing token");
    let pad = get('_')?;
    let mut ids = vec![get('^')?, pad];
    for c in ps.chars() {
        if let Some(id) = vocab.get(&c) {
            ids.extend([*id, pad]);
        }
    }
    ids.push(get('$')?);
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn kokoro_unpadded_length_selects_style_and_pieces_respect_limits() {
        assert_eq!(
            style_offset(3, 12, 510, 256, 54).unwrap(),
            (3 * 510 + 12) * 256
        );
        assert!(style_offset(54, 12, 510, 256, 54).is_err());
        assert!(style_offset(3, 510, 510, 256, 54).is_err());
        let v = tokens("a 1\nb 2\n. 3\n! 4\n; 5").unwrap();
        assert_eq!(
            kokoro_pieces("ab. ab! aaaa;", &v, 3).unwrap(),
            vec![vec![1, 2, 3], vec![1, 2, 4], vec![1, 1, 1], vec![1, 5]]
        );
    }
    #[test]
    fn piper_ids_all_concatenates_framed_sentences() {
        // Sherpa ran all sentences of a chunk in one model call with the
        // per-sentence framing kept in the concatenated sequence.
        let v = tokens("_ 0\n^ 1\n$ 2\n3\nɑ 4\nb 5\n. 6").unwrap();
        let one = piper_ids("ɑ", &v).unwrap();
        let two = piper_ids("b.", &v).unwrap();
        let mut want = one.clone();
        want.extend(two);
        assert_eq!(piper_ids_all(&["ɑ".into(), "b.".into()], &v).unwrap(), want);
        assert_eq!(piper_ids_all(&["ɑ".into()], &v).unwrap(), one);
        assert!(piper_ids_all(&[], &v).unwrap().is_empty());
    }
    #[test]
    fn tokens_space_unicode_and_framing() {
        let v = tokens("_ 0\n^ 1\n$ 2\n3\nɑ 4\n").unwrap();
        assert_eq!(v[&' '], 3);
        assert_eq!(piper_ids("ɑ ?", &v).unwrap(), vec![1, 0, 4, 0, 3, 0, 2]);
        assert!(tokens("a 1\na 2").is_err());
        assert!(tokens("ab 1").is_err());
    }
    #[test]
    fn pause_values_fall_back_to_defaults_on_bad_settings() {
        // A malformed settings file must not fail synthesis: the app
        // already warned at startup and runs on defaults.
        let dir =
            std::env::temp_dir().join(format!("smart-voices-pause-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bad = dir.join("settings.toml");
        std::fs::write(&bad, "silence_scale = \"loud\"\n").unwrap();
        assert_eq!(pause_values_from(Some(&bad)), (0.2, 0.2, 0.01));
        assert_eq!(pause_values_from(None), (0.2, 0.2, 0.01));
        std::fs::write(&bad, "silence_scale = 0.5\n").unwrap();
        assert_eq!(pause_values_from(Some(&bad)).0, 0.5);
        std::fs::remove_dir_all(&dir).unwrap();
    }
    #[test]
    fn fallback_notice_is_shown_only_once() {
        let shown = std::sync::atomic::AtomicBool::new(false);
        assert!(take_fallback_notice(&shown));
        assert!(!take_fallback_notice(&shown));
        assert!(!take_fallback_notice(&shown));
    }
    #[test]
    fn silence_scales_leading_interior_trailing_and_extension() {
        let x = [0., 0., 0., 1., 0., 0., 0., 1., 0., 0., 0.];
        assert_eq!(
            scale_silence(&x, 10, 0.5, 0.2, 0.01),
            vec![0., 1., 0., 1., 0.]
        );
        assert_eq!(scale_silence(&x, 10, 2., 0.2, 0.01).len(), 20);
        // Interior threshold is inclusive, trailing threshold is exclusive.
        assert_eq!(
            scale_silence(&[0., 0., 1., 0., 0.], 10, 0.5, 0.2, 0.01),
            vec![0., 1., 0., 0.]
        );
        assert_eq!(scale_silence(&x, 10, 1., 0.2, 0.01), x);
    }
}
