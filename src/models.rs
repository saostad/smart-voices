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

/// A downloadable voice model: a Kokoro variant or a Persian Piper voice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelId {
    Kokoro(ModelVariant),
    Persian(PersianVoice),
}

impl ModelId {
    /// Directory name under the models root.
    pub fn dir_name(self) -> &'static str {
        match self {
            Self::Kokoro(variant) => variant.dir_name(),
            Self::Persian(voice) => voice.dir_name(),
        }
    }

    /// Download URL: the `.tar.bz2` archive, or the single `.onnx` for a
    /// voice with a [`DownloadSpec`].
    pub fn url(self) -> &'static str {
        match self {
            Self::Kokoro(variant) => variant.url(),
            Self::Persian(voice) => voice.url(),
        }
    }

    /// Human-readable label for download progress.
    pub fn label(self) -> &'static str {
        match self {
            Self::Kokoro(variant) => variant.label(),
            Self::Persian(PersianVoice::PocketV2) => "Persian voice (Pocket v2, best quality)",
            Self::Persian(PersianVoice::Pocket) => "Persian voice (Pocket v1)",
            Self::Persian(PersianVoice::Amir) => "Persian voice (Amir)",
            Self::Persian(PersianVoice::Gyro) => "Persian voice (Gyro)",
            Self::Persian(PersianVoice::RezaIbrahim) => "Persian voice (Reza Ibrahim)",
            Self::Persian(PersianVoice::Ganji) => "Persian voice (Ganji)",
            Self::Persian(PersianVoice::GanjiAdabi) => "Persian voice (Ganji Adabi)",
        }
    }

    /// Files and dirs that must exist inside the model dir (model file first).
    pub fn required_files(self) -> Vec<&'static str> {
        match self {
            Self::Kokoro(variant) => variant.required_files().to_vec(),
            Self::Persian(voice) => voice.required_files(),
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

/// Models needed to read `chunks`: the Kokoro `variant` when any chunk is
/// English, the Persian `voice` when any chunk is Persian (Kokoro first).
/// English-only text never asks for the Persian model.
#[must_use]
pub fn models_for_chunks(
    chunks: &[SpeechChunk],
    variant: ModelVariant,
    voice: PersianVoice,
) -> Vec<ModelId> {
    let mut models = Vec::with_capacity(2);
    if chunks
        .iter()
        .any(|chunk| chunk_language(&chunk.text) == Lang::English)
    {
        models.push(ModelId::Kokoro(variant));
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
    if let ModelId::Persian(voice) = model
        && let Some(spec) = voice.download_spec()
    {
        return download_single_file(root, model, &spec, progress);
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

/// Download a single-file voice model (a Pocket voice): stream the `.onnx`
/// from `spec` into a staging file (resuming across dropped connections),
/// verify it by size and hash, then rename the staging dir into place.
fn download_single_file(
    root: &Path,
    model: ModelId,
    spec: &DownloadSpec,
    progress: impl FnMut(u64, Option<u64>),
) -> Result<PathBuf, ModelError> {
    let final_dir = model_dir(root, model);
    let partial = prepare_partial_dir(root, model)?;
    let cleanup = |partial: &Path| {
        let _ = std::fs::remove_dir_all(partial);
    };
    let file_name = model.required_files()[0];
    let staging_file = partial.join(file_name);
    if let Err(err) = download_file(
        spec.url,
        spec.size,
        spec.sha256,
        &staging_file,
        progress,
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
            ModelId::Kokoro(ModelVariant::Fast),
            ModelId::Kokoro(ModelVariant::Small),
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
            models_for_chunks(&[chunk("Hello."), chunk("World.")], variant, voice),
            [ModelId::Kokoro(variant)]
        );
        assert_eq!(
            models_for_chunks(&[chunk("سلام.")], variant, voice),
            [ModelId::Persian(voice)]
        );
        assert_eq!(
            models_for_chunks(&[chunk("سلام."), chunk("Hello.")], variant, voice),
            [ModelId::Kokoro(variant), ModelId::Persian(voice)]
        );
        assert!(models_for_chunks(&[], variant, voice).is_empty());
    }

    #[test]
    fn prepare_partial_dir_removes_leftover() {
        let dir = unique_temp_dir();
        let model = ModelId::Kokoro(ModelVariant::Fast);
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
