//! Device-free synthesis and audio encoding, shared by CLI clients.
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::player::{Synth, SynthesizedAudio};
use crate::settings::Settings;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Wav,
    Pcm,
    Flac,
    Mp3,
    Opus,
}
impl Format {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.to_ascii_lowercase().as_str() {
            "wav" => Ok(Self::Wav),
            "pcm" | "raw" => Ok(Self::Pcm),
            "flac" => Ok(Self::Flac),
            "mp3" => Ok(Self::Mp3),
            "opus" | "ogg" => Ok(Self::Opus),
            _ => Err(format!(
                "unknown audio format '{value}'; use wav, pcm, flac, mp3, opus or ogg"
            )),
        }
    }
    pub fn resolve(output: &str, explicit: Option<&str>) -> Result<Self, String> {
        let extension = Path::new(output).extension().and_then(|e| e.to_str());
        let inferred = extension.map(Self::parse).transpose()?;
        let selected = explicit.map(Self::parse).transpose()?;
        if let (Some(a), Some(b)) = (inferred, selected)
            && a != b
        {
            return Err("output extension does not match --format".into());
        }
        selected
            .or(inferred)
            .ok_or_else(|| "use --format when output has no extension (including stdout)".into())
    }
    pub fn check_encoder(self) -> Result<(), String> {
        if matches!(self, Self::Wav | Self::Pcm) {
            return Ok(());
        }
        let output = Command::new("ffmpeg")
            .args(["-hide_banner", "-encoders"])
            .output()
            .map_err(|e| format!("{self:?} requires ffmpeg on PATH: {e}"))?;
        let encoder = self.encoder();
        if !output.status.success()
            || !String::from_utf8_lossy(&output.stdout)
                .split_whitespace()
                .any(|s| s == encoder)
        {
            return Err(format!(
                "ffmpeg with encoder {encoder} is required for {self:?}"
            ));
        }
        Ok(())
    }
    fn encoder(self) -> &'static str {
        match self {
            Self::Flac => "flac",
            Self::Mp3 => "libmp3lame",
            Self::Opus => "libopus",
            _ => "pcm_s16le",
        }
    }
}

/// Shared installed-model preflight for device-free clients. Never downloads.
pub fn missing_models(
    root: &Path,
    chunks: &[crate::chunk::SpeechChunk],
    settings: &Settings,
) -> Vec<crate::models::ModelId> {
    crate::models::models_for_chunks(
        chunks,
        settings.model,
        settings.persian_voice,
        &settings.phonon_voice,
    )
    .into_iter()
    .filter(|model| !crate::models::is_installed(root, *model))
    .collect()
}

/// Render prepared chunks, selecting the engine by the same language detector
/// as playback. Mixed sample rates are resampled to the first chunk's rate
/// unless the caller explicitly requests an output rate.
pub fn render(
    text: &str,
    settings: &Settings,
    pronunciations: &crate::pronounce::Pronunciations,
    rate: Option<u32>,
    mut factory: impl FnMut(crate::lang::Lang) -> Result<Box<dyn Synth>, String>,
) -> Result<SynthesizedAudio, String> {
    let chunks = crate::pipeline::prepare(
        text,
        &crate::sanitize::SanitizerOptions::default(),
        pronunciations,
    );
    let mut english = None;
    let mut persian = None;
    render_chunks(
        &chunks,
        settings,
        pronunciations,
        rate,
        |language, text, voice, speed| {
            let slot = match language {
                crate::lang::Lang::English => &mut english,
                crate::lang::Lang::Persian => &mut persian,
            };
            if slot.is_none() {
                *slot = Some(factory(language)?);
            }
            slot.as_mut()
                .ok_or("engine not initialized")?
                .synthesize(text, voice, speed)
        },
    )
}

pub fn voice_for_language(settings: &Settings, language: crate::lang::Lang) -> &str {
    match language {
        crate::lang::Lang::Persian => settings.persian_voice.id(),
        crate::lang::Lang::English => match settings.model {
            crate::engine::ModelVariant::Phonon => &settings.phonon_voice,
            crate::engine::ModelVariant::Paradee => &settings.paradee_voice,
            _ => &settings.voice,
        },
    }
}

/// Render already prepared chunks; callers may retain engines across invocations.
pub fn render_chunks(
    chunks: &[crate::chunk::SpeechChunk],
    settings: &Settings,
    pronunciations: &crate::pronounce::Pronunciations,
    rate: Option<u32>,
    mut synthesize: impl FnMut(crate::lang::Lang, &str, &str, f32) -> Result<SynthesizedAudio, String>,
) -> Result<SynthesizedAudio, String> {
    settings.validate_strict()?;
    if rate == Some(0) {
        return Err("sample rate must be positive".into());
    }
    if chunks.is_empty() {
        return Err("input contains no speakable text".into());
    }
    crate::settings::with_invocation(settings, || {
        let mut result = SynthesizedAudio {
            samples: Vec::new(),
            sample_rate: rate.unwrap_or(0),
        };
        for chunk in chunks {
            let language = crate::lang::chunk_language(&chunk.text);
            let voice = voice_for_language(settings, language);
            let mut audio = synthesize(
                language,
                &pronunciations.apply(&chunk.text),
                voice,
                settings.speed,
            )?;
            if audio.sample_rate == 0 || audio.samples.iter().any(|s| !s.is_finite()) {
                return Err("engine returned invalid audio".into());
            }
            if result.sample_rate == 0 {
                result.sample_rate = audio.sample_rate;
            }
            // Playback appends this same semantic pause after each chunk.
            audio.samples.resize(
                audio.samples.len()
                    + (chunk.pause_after.seconds() * f64::from(audio.sample_rate)) as usize,
                0.,
            );
            result.samples.extend(crate::audio::resample_with(
                &audio.samples,
                audio.sample_rate,
                result.sample_rate,
                settings.resample_cutoff_ratio,
                settings.resample_filter_zeros,
            ));
        }
        Ok(result)
    })
}

/// Encode mono little-endian PCM (signed 16-bit or IEEE float32). Compressed
/// formats use ffmpeg explicitly; no format or encoder fallback is attempted.
pub fn encode(audio: &SynthesizedAudio, format: Format, float: bool) -> Result<Vec<u8>, String> {
    if audio.sample_rate == 0 || audio.samples.iter().any(|s| !s.is_finite()) {
        return Err("cannot encode invalid audio".into());
    }
    if float && !matches!(format, Format::Wav | Format::Pcm) {
        return Err("--float is supported only for WAV and raw PCM".into());
    }
    if matches!(format, Format::Wav | Format::Pcm) {
        let bytes = if float { 4u16 } else { 2u16 };
        let size = audio
            .samples
            .len()
            .checked_mul(usize::from(bytes))
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| *n <= u32::MAX - 36)
            .ok_or("audio exceeds WAV/PCM size limit")?;
        let mut out = Vec::new();
        if format == Format::Wav {
            out.extend(b"RIFF");
            out.extend((size + 36).to_le_bytes());
            out.extend(b"WAVEfmt ");
            out.extend(16u32.to_le_bytes());
            out.extend((if float { 3u16 } else { 1u16 }).to_le_bytes());
            out.extend(1u16.to_le_bytes());
            out.extend(audio.sample_rate.to_le_bytes());
            out.extend(
                audio
                    .sample_rate
                    .checked_mul(u32::from(bytes))
                    .ok_or("sample rate too large")?
                    .to_le_bytes(),
            );
            out.extend(bytes.to_le_bytes());
            out.extend((bytes * 8).to_le_bytes());
            out.extend(b"data");
            out.extend(size.to_le_bytes());
        }
        for sample in &audio.samples {
            if float {
                out.extend(sample.to_le_bytes());
            } else {
                out.extend(((sample.clamp(-1., 1.) * 32767.).round() as i16).to_le_bytes());
            }
        }
        return Ok(out);
    }
    format.check_encoder()?;
    let container = match format {
        Format::Flac => "flac",
        Format::Mp3 => "mp3",
        _ => "ogg",
    };
    let mut child = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "f32le",
            "-ar",
            &audio.sample_rate.to_string(),
            "-ac",
            "1",
            "-i",
            "pipe:0",
            "-c:a",
            format.encoder(),
            "-f",
            container,
            "pipe:1",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("couldn't start ffmpeg: {e}"))?;
    let mut input = child.stdin.take().ok_or("ffmpeg stdin unavailable")?;
    // Drain stdout concurrently so neither pipe can deadlock on long audio.
    let output = std::thread::scope(|scope| {
        let writer = scope.spawn(move || -> Result<(), String> {
            for sample in &audio.samples {
                input
                    .write_all(&sample.to_le_bytes())
                    .map_err(|e| format!("ffmpeg input failed: {e}"))?;
            }
            Ok(())
        });
        let output = child
            .wait_with_output()
            .map_err(|e| format!("ffmpeg failed: {e}"))?;
        let written = writer.join().map_err(|_| "ffmpeg input thread panicked")?;
        if !output.status.success() {
            return Err(format!(
                "ffmpeg encoding failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        written?;
        Ok(output.stdout)
    })?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_routes_languages_applies_dictionary_and_resamples() {
        use crate::lang::Lang;
        use std::sync::{Arc, Mutex};
        type Calls = Arc<Mutex<Vec<(String, String, f32)>>>;
        struct Fake {
            calls: Calls,
            rate: u32,
        }
        impl Synth for Fake {
            fn synthesize(
                &mut self,
                text: &str,
                voice: &str,
                speed: f32,
            ) -> Result<SynthesizedAudio, String> {
                let current = crate::settings::invocation_settings().expect("scoped settings");
                assert_eq!(current.silence_scale, 0.5);
                self.calls
                    .lock()
                    .unwrap()
                    .push((text.into(), voice.into(), speed));
                Ok(SynthesizedAudio {
                    samples: vec![0.25; 24],
                    sample_rate: self.rate,
                })
            }
        }
        let calls: Calls = Arc::default();
        let settings = Settings {
            silence_scale: 0.5,
            speed: 1.25,
            ..Settings::default()
        };
        let dict = crate::pronounce::Pronunciations::with_defaults();
        let text = "**SQL** is great.

سلام دنیا.";
        let chunks =
            crate::pipeline::prepare(text, &crate::sanitize::SanitizerOptions::default(), &dict);
        let mut expected = 0;
        for chunk in &chunks {
            let rate: u32 = match crate::lang::chunk_language(&chunk.text) {
                Lang::English => 24000,
                Lang::Persian => 16000,
            };
            let count = 24 + (chunk.pause_after.seconds() * f64::from(rate)) as usize;
            expected += (count as u64 * 24000).div_ceil(u64::from(rate)) as usize;
        }
        let audio = render(text, &settings, &dict, Some(24000), |lang| {
            Ok(Box::new(Fake {
                calls: calls.clone(),
                rate: if lang == Lang::English { 24000 } else { 16000 },
            }))
        })
        .unwrap();
        assert_eq!(audio.sample_rate, 24000);
        assert_eq!(audio.samples.len(), expected);
        let calls = calls.lock().unwrap();
        assert!(
            calls
                .iter()
                .any(|(text, voice, speed)| text.contains("sequel")
                    && voice == &settings.voice
                    && *speed == 1.25)
        );
        assert!(
            calls
                .iter()
                .any(|(_, voice, _)| voice == settings.persian_voice.id())
        );
        assert!(crate::settings::invocation_settings().is_none());
    }
    #[test]
    fn format_inference_rejects_ambiguity() {
        assert_eq!(Format::resolve("a.wav", None).unwrap(), Format::Wav);
        assert_eq!(Format::resolve("-", Some("ogg")).unwrap(), Format::Opus);
        assert!(Format::resolve("a.wav", Some("mp3")).is_err());
        assert!(Format::resolve("a.nope", Some("wav")).is_err());
        assert!(Format::resolve("-", None).is_err());
    }
    #[test]
    fn pcm_and_wav_samples_round_trip() {
        let audio = SynthesizedAudio {
            samples: vec![0., 0.5, -1., 1.],
            sample_rate: 24000,
        };
        for float in [false, true] {
            let raw = encode(&audio, Format::Pcm, float).unwrap();
            let wav = encode(&audio, Format::Wav, float).unwrap();
            assert_eq!(&wav[..4], b"RIFF");
            assert_eq!(&wav[8..16], b"WAVEfmt ");
            assert_eq!(&wav[44..], raw);
            assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 24000);
            if float {
                let decoded: Vec<f32> = raw
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| f32::from_le_bytes(*b))
                    .collect();
                assert_eq!(decoded, audio.samples);
            } else {
                assert_eq!(i16::from_le_bytes(raw[4..6].try_into().unwrap()), -32767);
            }
        }
        assert!(encode(&audio, Format::Mp3, true).is_err());
    }
}
