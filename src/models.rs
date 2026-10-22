//! Voice-model download and install state.

use std::error::Error;
use std::fmt;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bzip2::read::BzDecoder;
use tar::Archive;

use crate::chunk::SpeechChunk;
use crate::download::{DownloadError, PROGRESS_INTERVAL, download_file};
use crate::engine::{DownloadSpec, ModelVariant, PersianVoice};
use crate::lang::{Lang, chunk_language};
use crate::phonon::{DEFAULT_PHONON_VOICE, PHONON_VOICES, VOICES_DIR};

/// A downloadable voice model: an English variant, a Persian voice, or the
/// Phonon catalog voice bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelId {
    English(ModelVariant),
    Persian(PersianVoice),
    /// All 37 `gradium/phonon-voices` English voice files, downloaded
    /// together (only needed for a non-default Phonon voice).
    PhononVoices,
}

impl ModelId {
    /// Directory name under the models root.
    pub fn dir_name(self) -> &'static str {
        match self {
            Self::English(variant) => variant.dir_name(),
            Self::Persian(voice) => voice.dir_name(),
            Self::PhononVoices => VOICES_DIR,
        }
    }

    /// Download URL: the `.tar.bz2` archive, the single `.onnx` for a
    /// voice with a [`DownloadSpec`], or the model page for a variant with
    /// per-file downloads (used only in messages; never fetched).
    pub fn url(self) -> &'static str {
        match self {
            Self::English(variant) => variant.url(),
            Self::Persian(voice) => voice.url(),
            Self::PhononVoices => "https://huggingface.co/gradium/phonon-voices",
        }
    }

    /// Human-readable label for download progress.
    pub fn label(self) -> &'static str {
        match self {
            Self::English(variant) => variant.label(),
            Self::Persian(PersianVoice::PocketV2) => "Persian voice (Pocket v2, best quality)",
            Self::Persian(PersianVoice::Pocket) => "Persian voice (Pocket v1)",
            Self::Persian(PersianVoice::Amir) => "Persian voice (Amir)",
            Self::Persian(PersianVoice::Gyro) => "Persian voice (Gyro)",
            Self::Persian(PersianVoice::RezaIbrahim) => "Persian voice (Reza Ibrahim)",
            Self::Persian(PersianVoice::Ganji) => "Persian voice (Ganji)",
            Self::Persian(PersianVoice::GanjiAdabi) => "Persian voice (Ganji Adabi)",
            Self::PhononVoices => "Phonon voices (~9.5 MB)",
        }
    }

    /// Files and dirs that must exist inside the model dir (model file first).
    pub fn required_files(self) -> Vec<&'static str> {
        match self {
            Self::English(variant) => variant.required_files().to_vec(),
            Self::Persian(voice) => voice.required_files(),
            Self::PhononVoices => PHONON_VOICES.iter().map(|voice| voice.file.name).collect(),
        }
    }
}

/// Errors from downloading or installing a voice model.
#[derive(Debug)]
pub enum ModelError {
    /// The archive couldn't be fetched.
    Download { url: String, message: String },
    /// Streaming extraction into the staging dir failed.
    Unpack { dir: PathBuf, message: String },
    /// The extracted tree is missing a required file.
    Incomplete { path: PathBuf },
    /// The extracted tree couldn't be moved into place.
    Move { dir: PathBuf, message: String },
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModelError::Download { url, message } => {
                write!(f, "Couldn't download the voice model from {url}: {message}")
            }
            ModelError::Unpack { dir, message } => write!(
                f,
                "Couldn't download or unpack the voice model into {}: {message}",
                dir.display()
            ),
            ModelError::Incomplete { path } => write!(
                f,
                "The downloaded voice model is incomplete: {} is missing.",
                path.display()
            ),
            ModelError::Move { dir, message } => write!(
                f,
                "Couldn't move the voice model into place at {}: {message}",
                dir.display()
            ),
        }
    }
}

impl Error for ModelError {}

/// Models needed to read `chunks`: the English `variant` when any chunk is
/// English, the Persian `voice` when any chunk is Persian (English first).
/// English-only text never asks for the Persian model. With Phonon, the
/// catalog voice bundle follows the model unless the chosen Phonon voice is
/// the checkpoint's own (`default`) voice.
#[must_use]
pub fn models_for_chunks(
    chunks: &[SpeechChunk],
    variant: ModelVariant,
    voice: PersianVoice,
    phonon_voice: &str,
) -> Vec<ModelId> {
    let mut models = Vec::with_capacity(3);
    if chunks
        .iter()
        .any(|chunk| chunk_language(&chunk.text) == Lang::English)
    {
        models.push(ModelId::English(variant));
        if variant == ModelVariant::Phonon && phonon_voice != DEFAULT_PHONON_VOICE {
            models.push(ModelId::PhononVoices);
        }
    }
    if chunks
        .iter()
        .any(|chunk| chunk_language(&chunk.text) == Lang::Persian)
    {
        models.push(ModelId::Persian(voice));
    }
    models
}

/// Final install dir for a model: `root/<dir_name>`.
pub fn model_dir(root: &Path, model: ModelId) -> PathBuf {
    root.join(model.dir_name())
}

/// True when every required file is present in the install dir.
pub fn is_installed(root: &Path, model: ModelId) -> bool {
    let dir = model_dir(root, model);
    model
        .required_files()
        .iter()
        .all(|name| dir.join(name).exists())
}

/// Staging dir used while downloading: `root/.<dir_name>.partial`.
fn partial_path(root: &Path, model: ModelId) -> PathBuf {
    root.join(format!(".{}.partial", model.dir_name()))
}

/// Remove any leftover staging dir and create a fresh empty one.
fn prepare_partial_dir(root: &Path, model: ModelId) -> Result<PathBuf, ModelError> {
    let partial = partial_path(root, model);
    let unpack_err = |err: std::io::Error| ModelError::Unpack {
        dir: partial.clone(),
        message: err.to_string(),
    };
    match std::fs::remove_dir_all(&partial) {
        Ok(()) => {}
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => return Err(unpack_err(err)),
    }
    std::fs::create_dir_all(&partial).map_err(unpack_err)?;
    Ok(partial)
}

/// Download `model` into `root`, reporting `(bytes_done, total_bytes)`
/// progress. Streams the response through the bzip2 decoder straight into
/// the tar extractor (nothing is fully buffered in memory), verifies the
/// required files, then renames the tree into place. Any failure removes the
/// staging dir, so a half-extracted tree is never left behind.
pub fn download(
    root: &Path,
    model: ModelId,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<PathBuf, ModelError> {
    progress(0, None);
    if let ModelId::Persian(voice) = model
        && let Some(spec) = voice.download_spec()
    {
        let files = [(model.required_files()[0], spec)];
        return download_files(root, model, &files, progress);
    }
    if let ModelId::English(variant) = model {
        let files = variant.download_files();
        if !files.is_empty() {
            let files: Vec<(&'static str, DownloadSpec)> =
                files.iter().map(|file| (file.name, file.spec)).collect();
            return download_files(root, model, &files, progress);
        }
    }
    if model == ModelId::PhononVoices {
        let files: Vec<(&'static str, DownloadSpec)> = PHONON_VOICES
            .iter()
            .map(|voice| (voice.file.name, voice.file.spec))
            .collect();
        return download_files(root, model, &files, progress);
    }
    let final_dir = model_dir(root, model);
    let partial = prepare_partial_dir(root, model)?;
    let cleanup = |partial: &Path| {
        let _ = std::fs::remove_dir_all(partial);
    };

    let url = model.url();
    // Timeouts so a stalled connection can't hang forever; no overall
    // timeout since the file is ~350 MB. Proxy comes from the environment
    // (HTTP_PROXY/HTTPS_PROXY/ALL_PROXY, any case).
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(30))
        .try_proxy_from_env(true)
        .build();
    let response = agent.get(url).call().map_err(|err| {
        cleanup(&partial);
        ModelError::Download {
            url: url.to_string(),
            message: err.to_string(),
        }
    })?;
    let total: Option<u64> = response
        .header("Content-Length")
        .and_then(|value| value.parse().ok());
    progress(0, total);

    let (unpack_result, done) = {
        let reader = ProgressReader::new(response.into_reader(), total, &mut progress);
        let mut decoder = BzDecoder::new(reader);
        let result = Archive::new(&mut decoder).unpack(&partial);
        let done = decoder.into_inner().bytes_done();
        (result, done)
    };
    progress(done, total);
    if let Err(err) = unpack_result {
        cleanup(&partial);
        return Err(ModelError::Unpack {
            dir: partial.clone(),
            message: err.to_string(),
        });
    }

    // The sherpa tarballs wrap everything in a top-level `<dir_name>/` dir;
    // accept a flat tarball too.
    let nested = partial.join(model.dir_name());
    let source = if nested.is_dir() {
        nested
    } else {
        partial.clone()
    };
    for name in model.required_files() {
        if !source.join(name).exists() {
            cleanup(&partial);
            return Err(ModelError::Incomplete {
                path: final_dir.join(name),
            });
        }
    }

    match std::fs::remove_dir_all(&final_dir) {
        Ok(()) => {}
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => {
            cleanup(&partial);
            return Err(ModelError::Move {
                dir: final_dir.clone(),
                message: err.to_string(),
            });
        }
    }
    if let Err(err) = std::fs::rename(&source, &final_dir) {
        cleanup(&partial);
        return Err(ModelError::Move {
            dir: final_dir.clone(),
            message: err.to_string(),
        });
    }
    // Drops the staging remainder (the emptied partial dir when the tarball
    // had a top-level dir; a no-op otherwise).
    cleanup(&partial);
    Ok(final_dir)
}

/// Download one file per `files` entry (a Pocket voice is the one-entry
/// case): stream each file into the same staging dir (resuming across
/// dropped connections), verify it by size and hash, then rename the staging
/// dir into place. Progress is cumulative: bytes of finished files plus the
/// current file's bytes, over the sum of all sizes. Any failure removes the
/// staging dir.
fn download_files(
    root: &Path,
    model: ModelId,
    files: &[(&'static str, DownloadSpec)],
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<PathBuf, ModelError> {
    let final_dir = model_dir(root, model);
    let partial = prepare_partial_dir(root, model)?;
    let cleanup = |partial: &Path| {
        let _ = std::fs::remove_dir_all(partial);
    };
    let total: u64 = files.iter().map(|(_, spec)| spec.size).sum();
    progress(0, Some(total));
    let mut finished: u64 = 0;
    for (name, spec) in files {
        let staging_file = partial.join(name);
        if let Err(err) = download_file(
            spec.url,
            spec.size,
            spec.sha256,
            &staging_file,
            |done, _| progress(finished + done, Some(total)),
            || false,
        ) {
            cleanup(&partial);
            return Err(match err {
                DownloadError::Fetch { url, message } => ModelError::Download { url, message },
                DownloadError::Verify { message } => ModelError::Unpack {
                    dir: partial.clone(),
                    message,
                },
            });
        }
        finished += spec.size;
    }
    match std::fs::remove_dir_all(&final_dir) {
        Ok(()) => {}
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => {
            cleanup(&partial);
            return Err(ModelError::Move {
                dir: final_dir.clone(),
                message: err.to_string(),
            });
        }
    }
    if let Err(err) = std::fs::rename(&partial, &final_dir) {
        cleanup(&partial);
        return Err(ModelError::Move {
            dir: final_dir.clone(),
            message: err.to_string(),
        });
    }
    Ok(final_dir)
}

/// `Read` adapter reporting download progress, throttled to ~10×/s.
struct ProgressReader<R, F: FnMut(u64, Option<u64>)> {
    inner: R,
    total: Option<u64>,
    done: u64,
    last_report: Instant,
    progress: F,
}

impl<R: Read, F: FnMut(u64, Option<u64>)> ProgressReader<R, F> {
    fn new(inner: R, total: Option<u64>, progress: F) -> Self {
        Self {
            inner,
            total,
            done: 0,
            last_report: Instant::now(),
            progress,
        }
    }

    fn bytes_done(&self) -> u64 {
        self.done
    }
}

impl<R: Read, F: FnMut(u64, Option<u64>)> Read for ProgressReader<R, F> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.done += n as u64;
        if n > 0 && self.last_report.elapsed() >= PROGRESS_INTERVAL {
            (self.progress)(self.done, self.total);
            self.last_report = Instant::now();
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_temp_dir() -> PathBuf {
        let id = TEST_DIR_COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!(
            "smart-voices-models-test-{}-{id}",
            std::process::id()
        ))
    }

    /// Create stub files for every required entry (dirs as dirs).
    fn stub_install(root: &Path, model: ModelId) {
        let dir = model_dir(root, model);
        for name in model.required_files() {
            let path = dir.join(name);
            if name == "espeak-ng-data" || name == "dict" {
                std::fs::create_dir_all(&path).unwrap();
            } else {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, []).unwrap();
            }
        }
    }

    fn all_models() -> Vec<ModelId> {
        let mut models = vec![
            ModelId::English(ModelVariant::Fast),
            ModelId::English(ModelVariant::Small),
            ModelId::English(ModelVariant::Phonon),
            ModelId::PhononVoices,
        ];
        models.extend(
            crate::engine::PERSIAN_VOICES
                .iter()
                .map(|voice| ModelId::Persian(*voice)),
        );
        models
    }

    #[test]
    fn is_installed_false_on_empty_dir() {
        for model in all_models() {
            let dir = unique_temp_dir();
            std::fs::create_dir_all(&dir).unwrap();
            assert!(!is_installed(&dir, model), "model {model:?}");
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }

    #[test]
    fn is_installed_true_with_stub_files() {
        for model in all_models() {
            let dir = unique_temp_dir();
            stub_install(&dir, model);
            assert!(is_installed(&dir, model), "model {model:?}");
            // One missing file flips it back.
            let first = model.required_files()[0];
            std::fs::remove_file(model_dir(&dir, model).join(first)).unwrap();
            assert!(!is_installed(&dir, model), "model {model:?}");
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }

    #[test]
    fn models_for_chunks_requests_only_needed_models() {
        use crate::sanitize::SpeechPause;

        let chunk = |text: &str| SpeechChunk {
            text: text.to_string(),
            pause_after: SpeechPause::None,
        };
        let variant = ModelVariant::Small;
        let voice = PersianVoice::Gyro;
        assert_eq!(
            models_for_chunks(
                &[chunk("Hello."), chunk("World.")],
                variant,
                voice,
                DEFAULT_PHONON_VOICE
            ),
            [ModelId::English(variant)]
        );
        assert_eq!(
            models_for_chunks(&[chunk("سلام.")], variant, voice, DEFAULT_PHONON_VOICE),
            [ModelId::Persian(voice)]
        );
        assert_eq!(
            models_for_chunks(
                &[chunk("سلام."), chunk("Hello.")],
                variant,
                voice,
                DEFAULT_PHONON_VOICE
            ),
            [ModelId::English(variant), ModelId::Persian(voice)]
        );
        assert!(models_for_chunks(&[], variant, voice, DEFAULT_PHONON_VOICE).is_empty());
    }

    #[test]
    fn models_for_chunks_adds_phonon_voices_only_for_catalog_voice() {
        use crate::sanitize::SpeechPause;

        let chunk = |text: &str| SpeechChunk {
            text: text.to_string(),
            pause_after: SpeechPause::None,
        };
        let english = [chunk("Hello.")];
        let voice = PersianVoice::Gyro;
        // The checkpoint's own voice needs no bundle.
        assert_eq!(
            models_for_chunks(&english, ModelVariant::Phonon, voice, DEFAULT_PHONON_VOICE),
            [ModelId::English(ModelVariant::Phonon)]
        );
        // A catalog voice (Zoey) pulls the bundle right after the model.
        assert_eq!(
            models_for_chunks(&english, ModelVariant::Phonon, voice, "NbpkqMVS3CJeq2j8"),
            [
                ModelId::English(ModelVariant::Phonon),
                ModelId::PhononVoices
            ]
        );
        // The Kokoro variants never pull the bundle, whatever the voice id.
        assert_eq!(
            models_for_chunks(&english, ModelVariant::Fast, voice, "NbpkqMVS3CJeq2j8"),
            [ModelId::English(ModelVariant::Fast)]
        );
        // Persian-only text with Phonon selected stays Persian-only.
        assert_eq!(
            models_for_chunks(
                &[chunk("سلام.")],
                ModelVariant::Phonon,
                voice,
                "NbpkqMVS3CJeq2j8"
            ),
            [ModelId::Persian(voice)]
        );
    }

    #[test]
    fn prepare_partial_dir_removes_leftover() {
        let dir = unique_temp_dir();
        let model = ModelId::English(ModelVariant::Fast);
        // Simulate a leftover from an interrupted run.
        let leftover = partial_path(&dir, model);
        std::fs::create_dir_all(leftover.join("nested")).unwrap();
        std::fs::write(leftover.join("nested").join("junk.bin"), [1, 2, 3]).unwrap();

        let partial = prepare_partial_dir(&dir, model).unwrap();
        assert_eq!(partial, dir.join(format!(".{}.partial", model.dir_name())));
        assert!(partial.is_dir());
        assert_eq!(std::fs::read_dir(&partial).unwrap().count(), 0);

        // And it creates the dir when nothing was there.
        std::fs::remove_dir_all(&dir).unwrap();
        let partial = prepare_partial_dir(&dir, model).unwrap();
        assert!(partial.is_dir());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
