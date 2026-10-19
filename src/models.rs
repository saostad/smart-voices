//! Voice-model download and install state.

use std::error::Error;
use std::fmt;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bzip2::read::BzDecoder;
use tar::Archive;

use crate::chunk::SpeechChunk;
use crate::engine::{ModelVariant, PersianVoice};
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

    /// Download URL of the `.tar.bz2` archive.
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
            Self::Persian(PersianVoice::Pocket) => "Persian voice (Pocket, best quality)",
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

    /// True for the Pocket model, which downloads as a single `.onnx` file
    /// rather than a `.tar.bz2` archive.
    pub fn is_pocket(self) -> bool {
        matches!(self, Self::Persian(PersianVoice::Pocket))
    }
}

/// Minimum time between progress callback invocations (at most ~10×/s).
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

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
    if model.is_pocket() {
        return download_single_file(root, model, progress);
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

/// Download the Pocket model: one `.onnx` file streamed through SHA-256 into
/// a staging file (resuming across dropped connections), verified by size
/// and hash, then renamed into place.
fn download_single_file(
    root: &Path,
    model: ModelId,
    progress: impl FnMut(u64, Option<u64>),
) -> Result<PathBuf, ModelError> {
    let final_dir = model_dir(root, model);
    let partial = prepare_partial_dir(root, model)?;
    let cleanup = |partial: &Path| {
        let _ = std::fs::remove_dir_all(partial);
    };
    let url = model.url();
    // Timeouts so a stalled connection can't hang forever; no overall
    // timeout since the file is ~226 MB. Proxy comes from the environment
    // (HTTP_PROXY/HTTPS_PROXY/ALL_PROXY, any case).
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(30))
        .try_proxy_from_env(true)
        .build();
    let opener = |from_byte: u64| ureq_opener(&agent, url, from_byte);
    let file_name = model.required_files()[0];
    let staging_file = partial.join(file_name);
    let expected_size = PersianVoice::POCKET_SIZE_BYTES;
    let mut hooks = FetchHooks {
        progress,
        cancelled: || false,
        sleep: std::thread::sleep,
        on_info: |message: &str| eprintln!("{message}"),
    };
    let (done, hasher) =
        match stream_download_to_file(&opener, url, &staging_file, expected_size, &mut hooks) {
            Ok(ok) => ok,
            Err(err) => {
                cleanup(&partial);
                return Err(err);
            }
        };
    if let Err(err) = verify_download(
        done,
        hasher,
        expected_size,
        PersianVoice::POCKET_SHA256,
        &partial,
    ) {
        cleanup(&partial);
        return Err(err);
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

/// Back-off between single-file resume attempts: 5 consecutive failures with
/// no progress, then give up (like readly's `RESUME_DELAYS_MS`).
const RESUME_DELAYS: [Duration; 5] = [
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
    Duration::from_secs(16),
    Duration::from_secs(30),
];

/// Read buffer for the single-file download (like readly's 64 KB).
const READ_BUFFER_BYTES: usize = 64 * 1024;

/// Minimal HTTP response. The download talks to this instead of `ureq`
/// directly so unit tests can serve fixture bodies without a network.
struct HttpResponse<R> {
    status: u16,
    status_message: String,
    /// Start byte of the response's `Content-Range` header, if present.
    content_range_start: Option<u64>,
    body: R,
}

/// Progress/cancel/sleep/log hooks for [`stream_download_to_file`], bundled
/// so tests can inject fakes.
struct FetchHooks<Progress, Cancelled, Sleep, OnInfo> {
    progress: Progress,
    cancelled: Cancelled,
    sleep: Sleep,
    on_info: OnInfo,
}

/// Retry state for [`stream_download_to_file`].
#[derive(Default)]
struct ResumeState {
    failures: usize,
    done_at_last_failure: u64,
    resume_from: u64,
}

/// Bytes hashed and written so far, plus the progress throttle.
struct StreamProgress {
    hasher: ring::digest::Context,
    done: u64,
    last_report: Instant,
}

/// What one response body did: reached EOF, or failed mid-stream (resume).
enum BodyOutcome {
    Complete,
    Failed(std::io::Error),
}

/// Start byte of a `Content-Range: bytes <start>-<end>/<total>` value.
fn parse_content_range_start(value: Option<&str>) -> Option<u64> {
    let value = value?.trim();
    // Kotlin's `removePrefix` keeps the string when the prefix is absent, so
    // a bare `<start>-<end>/<total>` parses too.
    let range = value.strip_prefix("bytes ").unwrap_or(value);
    range.split('-').next()?.parse().ok()
}

/// Production opener backed by `ureq`: opens `url` from `from_byte` (0 = the
/// whole file, no `Range` header). HTTP error statuses come back as a
/// response (terminal, never retried); transport failures come back as an
/// I/O error.
fn ureq_opener(
    agent: &ureq::Agent,
    url: &str,
    from_byte: u64,
) -> Result<HttpResponse<Box<dyn Read + Send + Sync>>, std::io::Error> {
    let mut request = agent.get(url);
    if from_byte > 0 {
        request = request.set("Range", &format!("bytes={from_byte}-"));
    }
    let response = match request.call() {
        Ok(response) => response,
        Err(ureq::Error::Status(code, response)) => {
            return Ok(HttpResponse {
                status: code,
                status_message: response.status_text().to_string(),
                content_range_start: parse_content_range_start(response.header("Content-Range")),
                body: Box::new(std::io::empty()) as Box<dyn Read + Send + Sync>,
            });
        }
        Err(err) => return Err(std::io::Error::other(err)),
    };
    Ok(HttpResponse {
        status: response.status(),
        status_message: response.status_text().to_string(),
        content_range_start: parse_content_range_start(response.header("Content-Range")),
        body: response.into_reader(),
    })
}

/// The shared HTTP-failure error unless `status` is 2xx.
fn throw_if_http_error(url: &str, status: u16, status_message: &str) -> Result<(), ModelError> {
    if !(200..300).contains(&status) {
        let detail = if status_message.trim().is_empty() {
            String::new()
        } else {
            format!(" {status_message}")
        };
        return Err(ModelError::Download {
            url: url.to_string(),
            message: format!("Voice download failed: HTTP {status}{detail} from {url}"),
        });
    }
    Ok(())
}

/// [`sleep`] in ≤ 250 ms slices, checking `cancelled` between them so a
/// cancel during a long back-off returns within one slice. True when
/// cancelled.
fn cancel_aware_sleep(
    total: Duration,
    cancelled: &impl Fn() -> bool,
    sleep: &impl Fn(Duration),
) -> bool {
    let mut remaining = total;
    let slice = Duration::from_millis(250);
    while remaining > Duration::ZERO {
        if cancelled() {
            return true;
        }
        let step = remaining.min(slice);
        sleep(step);
        remaining -= step;
    }
    cancelled()
}

/// One failed attempt: reset the streak when bytes arrived since the last
/// failure, else back off (cancel-aware) and resume from `done`. After 5
/// attempts with no progress, fail.
fn handle_resume_failure<Cancelled, Sleep, OnInfo>(
    url: &str,
    message: &str,
    done: u64,
    expected_size: u64,
    state: &mut ResumeState,
    hooks: &mut FetchHooks<impl FnMut(u64, Option<u64>), Cancelled, Sleep, OnInfo>,
) -> Result<(), ModelError>
where
    Cancelled: Fn() -> bool,
    Sleep: Fn(Duration),
    OnInfo: FnMut(&str),
{
    if done > state.done_at_last_failure {
        // Bytes arrived since the last failure: this starts a new streak.
        state.failures = 0;
        state.done_at_last_failure = done;
    }
    state.failures += 1;
    if state.failures > RESUME_DELAYS.len() {
        return Err(ModelError::Download {
            url: url.to_string(),
            message: format!(
                "Voice download failed after {} retries at {done} of {expected_size} bytes: {message}",
                RESUME_DELAYS.len()
            ),
        });
    }
    (hooks.on_info)(&format!(
        "Voice download interrupted at {done} of {expected_size} bytes ({message}); resuming (attempt {} of {})",
        state.failures,
        RESUME_DELAYS.len()
    ));
    if cancel_aware_sleep(RESUME_DELAYS[state.failures - 1], &hooks.cancelled, &hooks.sleep) {
        return Err(ModelError::Download {
            url: url.to_string(),
            message: "Voice download cancelled".to_string(),
        });
    }
    state.resume_from = done;
    Ok(())
}

/// Copies one response body into `target` (appending when `append`), hashing
/// every byte. Transport and file errors come back as [`BodyOutcome::Failed`]
/// for the resume loop (like readly, which retries any `IOException` past
/// the first open); cancel comes back as `Err`.
fn copy_response_body<R: Read, Progress, Cancelled, Sleep, OnInfo>(
    body: &mut R,
    url: &str,
    target: &Path,
    append: bool,
    expected_size: u64,
    progress_state: &mut StreamProgress,
    hooks: &mut FetchHooks<Progress, Cancelled, Sleep, OnInfo>,
) -> Result<BodyOutcome, ModelError>
where
    Progress: FnMut(u64, Option<u64>),
    Cancelled: Fn() -> bool,
    Sleep: Fn(Duration),
    OnInfo: FnMut(&str),
{
    use std::io::Write as _;

    let mut out = match std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .open(target)
    {
        Ok(file) => file,
        Err(err) => return Ok(BodyOutcome::Failed(err)),
    };
    let mut buf = [0u8; READ_BUFFER_BYTES];
    loop {
        if (hooks.cancelled)() {
            return Err(ModelError::Download {
                url: url.to_string(),
                message: "Voice download cancelled".to_string(),
            });
        }
        match body.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if let Err(err) = out.write_all(&buf[..n]) {
                    return Ok(BodyOutcome::Failed(err));
                }
                progress_state.hasher.update(&buf[..n]);
                progress_state.done += n as u64;
                if progress_state.last_report.elapsed() >= PROGRESS_INTERVAL {
                    (hooks.progress)(progress_state.done, Some(expected_size));
                    progress_state.last_report = Instant::now();
                }
            }
            Err(err) => return Ok(BodyOutcome::Failed(err)),
        }
    }
    if let Err(err) = out.flush() {
        return Ok(BodyOutcome::Failed(err));
    }
    Ok(BodyOutcome::Complete)
}

/// Streams one file from HTTP through the SHA-256 hash into `target`, like
/// readly's `streamDownloadToFile`. A dropped connection resumes instead of
/// restarting: past the very first open, an I/O error waits out the back-off
/// (cancel-aware), reopens from the bytes already written, and appends,
/// keeping the same hash. A 206 whose `Content-Range` start matches the
/// offset continues; a 200 truncates the file, resets the hash and restarts
/// from 0. Reconnect failures are retried too; HTTP error statuses and
/// `Content-Range` mismatches are terminal and never retried. Returns the
/// bytes consumed and the running hash (covering exactly the file's bytes).
fn stream_download_to_file<R: Read, Progress, Cancelled, Sleep, OnInfo>(
    opener: &impl Fn(u64) -> Result<HttpResponse<R>, std::io::Error>,
    url: &str,
    target: &Path,
    expected_size: u64,
    hooks: &mut FetchHooks<Progress, Cancelled, Sleep, OnInfo>,
) -> Result<(u64, ring::digest::Context), ModelError>
where
    Progress: FnMut(u64, Option<u64>),
    Cancelled: Fn() -> bool,
    Sleep: Fn(Duration),
    OnInfo: FnMut(&str),
{
    let mut state = ResumeState::default();
    let mut progress_state = StreamProgress {
        hasher: ring::digest::Context::new(&ring::digest::SHA256),
        done: 0,
        last_report: Instant::now(),
    };
    (hooks.progress)(0, Some(expected_size));
    loop {
        if (hooks.cancelled)() {
            return Err(ModelError::Download {
                url: url.to_string(),
                message: "Voice download cancelled".to_string(),
            });
        }
        // `failures` is only 0 before the first failure, so this is true for
        // the very first open alone: a reopen from 0 after a 200 restart
        // stays retryable, as does one after a 0-byte first read.
        let first_open = state.failures == 0 && state.resume_from == 0;
        let response = match opener(state.resume_from) {
            Ok(response) => response,
            Err(err) => {
                if first_open {
                    return Err(ModelError::Download {
                        url: url.to_string(),
                        message: err.to_string(),
                    });
                }
                handle_resume_failure(
                    url,
                    &err.to_string(),
                    progress_state.done,
                    expected_size,
                    &mut state,
                    hooks,
                )?;
                continue;
            }
        };
        if state.resume_from == 0 {
            throw_if_http_error(url, response.status, &response.status_message)?;
        } else if response.status == 200 {
            progress_state.hasher = ring::digest::Context::new(&ring::digest::SHA256);
            progress_state.done = 0;
            progress_state.last_report = Instant::now();
            state.resume_from = 0;
            state.done_at_last_failure = 0;
            (hooks.progress)(0, Some(expected_size));
        } else {
            if response.status != 206 || response.content_range_start != Some(state.resume_from) {
                throw_if_http_error(url, response.status, &response.status_message)?;
                return Err(ModelError::Download {
                    url: url.to_string(),
                    message: format!(
                        "Voice download failed: HTTP {} with an unexpected Content-Range for resume from {} bytes",
                        response.status, state.resume_from
                    ),
                });
            }
        }
        let append = state.resume_from > 0;
        let mut body = response.body;
        match copy_response_body(
            &mut body,
            url,
            target,
            append,
            expected_size,
            &mut progress_state,
            hooks,
        )? {
            BodyOutcome::Complete => {
                (hooks.progress)(progress_state.done, Some(expected_size));
                return Ok((progress_state.done, progress_state.hasher));
            }
            BodyOutcome::Failed(err) => {
                handle_resume_failure(
                    url,
                    &err.to_string(),
                    progress_state.done,
                    expected_size,
                    &mut state,
                    hooks,
                )?;
            }
        }
    }
}

/// Lowercase hex of `bytes` (the `ring` digest has no hex formatter).
fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0xf) as usize] as char);
    }
    out
}

/// Size and hash checks after the stream: `consumed` must equal
/// `expected_size` and the running hash must equal `expected_sha256`.
fn verify_download(
    consumed: u64,
    hasher: ring::digest::Context,
    expected_size: u64,
    expected_sha256: &str,
    staging_dir: &Path,
) -> Result<(), ModelError> {
    if consumed != expected_size {
        return Err(ModelError::Unpack {
            dir: staging_dir.to_path_buf(),
            message: format!(
                "Voice download is incomplete: expected {expected_size} bytes, got {consumed}"
            ),
        });
    }
    let actual = hex_lower(hasher.finish().as_ref());
    if !actual.eq_ignore_ascii_case(expected_sha256) {
        return Err(ModelError::Unpack {
            dir: staging_dir.to_path_buf(),
            message: format!(
                "Voice download is corrupted: expected SHA-256 {expected_sha256}, got {actual}"
            ),
        });
    }
    Ok(())
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

    /// One scripted `Read` segment: bytes, or a failure.
    enum Seg {
        Bytes(Vec<u8>),
        Fail(String),
    }

    /// A `Read` serving scripted bytes, then EOF or a failure.
    struct SegReader {
        segments: Vec<Seg>,
        index: usize,
        pos: usize,
    }

    impl SegReader {
        fn bytes(data: &[u8]) -> Self {
            Self {
                segments: vec![Seg::Bytes(data.to_vec())],
                index: 0,
                pos: 0,
            }
        }

        fn bytes_then_fail(data: &[u8], message: &str) -> Self {
            Self {
                segments: vec![Seg::Bytes(data.to_vec()), Seg::Fail(message.to_string())],
                index: 0,
                pos: 0,
            }
        }

        fn fail(message: &str) -> Self {
            Self {
                segments: vec![Seg::Fail(message.to_string())],
                index: 0,
                pos: 0,
            }
        }
    }

    impl Read for SegReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            loop {
                match self.segments.get(self.index) {
                    None => return Ok(0),
                    Some(Seg::Fail(_)) => {
                        let Some(Seg::Fail(message)) = self.segments.get(self.index) else {
                            unreachable!()
                        };
                        let message = message.clone();
                        self.index += 1;
                        return Err(std::io::Error::other(message));
                    }
                    Some(Seg::Bytes(data)) => {
                        if self.pos >= data.len() {
                            self.index += 1;
                            self.pos = 0;
                            continue;
                        }
                        let n = buf.len().min(data.len() - self.pos);
                        buf[..n].copy_from_slice(&data[self.pos..self.pos + n]);
                        self.pos += n;
                        return Ok(n);
                    }
                }
            }
        }
    }

    struct ScriptedResponse {
        status: u16,
        status_message: String,
        content_range_start: Option<u64>,
        body: SegReader,
    }

    impl ScriptedResponse {
        fn ok(body: SegReader) -> Self {
            Self {
                status: 200,
                status_message: "OK".to_string(),
                content_range_start: None,
                body,
            }
        }

        fn partial(start: u64, body: SegReader) -> Self {
            Self {
                status: 206,
                status_message: "Partial Content".to_string(),
                content_range_start: Some(start),
                body,
            }
        }
    }

    /// An injected HTTP opener serving one scripted response (or open
    /// failure) per call, recording the requested offsets.
    struct ScriptOpener {
        script: std::cell::RefCell<std::collections::VecDeque<Result<ScriptedResponse, String>>>,
        opens: std::cell::RefCell<Vec<u64>>,
    }

    impl ScriptOpener {
        fn new(script: Vec<Result<ScriptedResponse, String>>) -> Self {
            Self {
                script: std::cell::RefCell::new(script.into_iter().collect()),
                opens: std::cell::RefCell::new(Vec::new()),
            }
        }

        fn opener(&self) -> impl Fn(u64) -> Result<HttpResponse<SegReader>, std::io::Error> + '_ {
            |from_byte| {
                self.opens.borrow_mut().push(from_byte);
                match self.script.borrow_mut().pop_front() {
                    Some(Ok(response)) => Ok(HttpResponse {
                        status: response.status,
                        status_message: response.status_message,
                        content_range_start: response.content_range_start,
                        body: response.body,
                    }),
                    Some(Err(message)) => Err(std::io::Error::other(message)),
                    None => panic!("opener called more times than scripted"),
                }
            }
        }
    }

    fn sha256_hex(data: &[u8]) -> String {
        let mut context = ring::digest::Context::new(&ring::digest::SHA256);
        context.update(data);
        hex_lower(context.finish().as_ref())
    }

    const TEST_URL: &str = "https://example.invalid/pocket-farsi-v1.onnx";

    /// Total slept across ≤ 250 ms back-off slices.
    fn total_slept(sleeps: &[Duration]) -> Duration {
        assert!(
            sleeps.iter().all(|delay| *delay <= Duration::from_millis(250)),
            "back-off sleeps in slices: {sleeps:?}"
        );
        sleeps.iter().sum()
    }


    #[test]
    fn parses_content_range_start() {
        assert_eq!(
            parse_content_range_start(Some("bytes 6-10/11")),
            Some(6)
        );
        // Kotlin's `removePrefix` keeps a bare range as-is.
        assert_eq!(parse_content_range_start(Some("6-10/11")), Some(6));
        assert_eq!(parse_content_range_start(None), None);
        assert_eq!(parse_content_range_start(Some("bytes */11")), None);
        assert_eq!(parse_content_range_start(Some("garbage")), None);
    }

    #[test]
    fn pocket_download_resumes_on_206() {
        let dir = unique_temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("pocket-farsi-v1.onnx");
        let script = ScriptOpener::new(vec![
            Ok(ScriptedResponse::ok(SegReader::bytes_then_fail(
                b"hello ",
                "connection reset",
            ))),
            Ok(ScriptedResponse::partial(6, SegReader::bytes(b"world"))),
        ]);
        let info = std::cell::RefCell::new(Vec::new());
        let sleeps = std::cell::RefCell::new(Vec::new());
        let mut hooks = FetchHooks {
                progress: |_: u64, _: Option<u64>| {},
                cancelled: || false,
                sleep: |delay| sleeps.borrow_mut().push(delay),
                on_info: |message: &str| info.borrow_mut().push(message.to_string()),
        };

        let (done, hasher) =
            stream_download_to_file(&script.opener(), TEST_URL, &target, 11, &mut hooks).unwrap();

        assert_eq!(done, 11);
        assert_eq!(std::fs::read(&target).unwrap(), b"hello world");
        verify_download(done, hasher, 11, &sha256_hex(b"hello world"), &dir).unwrap();
        assert_eq!(*script.opens.borrow(), vec![0, 6]);
        assert_eq!(total_slept(&sleeps.borrow()), Duration::from_secs(2));
        assert_eq!(
            *info.borrow(),
            vec!["Voice download interrupted at 6 of 11 bytes (connection reset); resuming (attempt 1 of 5)".to_string()]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn pocket_download_restarts_on_200() {
        let dir = unique_temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("pocket-farsi-v1.onnx");
        let script = ScriptOpener::new(vec![
            Ok(ScriptedResponse::ok(SegReader::bytes_then_fail(
                b"hello ",
                "connection reset",
            ))),
            // No range support: the whole file again from 0.
            Ok(ScriptedResponse::ok(SegReader::bytes(b"hello world"))),
        ]);
        let mut hooks = FetchHooks {
                progress: |_: u64, _: Option<u64>| {},
                cancelled: || false,
                sleep: |_| {},
                on_info: |_: &str| {},
        };

        let (done, hasher) =
            stream_download_to_file(&script.opener(), TEST_URL, &target, 11, &mut hooks).unwrap();

        assert_eq!(done, 11);
        assert_eq!(std::fs::read(&target).unwrap(), b"hello world");
        verify_download(done, hasher, 11, &sha256_hex(b"hello world"), &dir).unwrap();
        assert_eq!(*script.opens.borrow(), vec![0, 6]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn pocket_download_resets_the_streak_when_bytes_arrive() {
        let dir = unique_temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("pocket-farsi-v1.onnx");
        let script = ScriptOpener::new(vec![
            Ok(ScriptedResponse::ok(SegReader::bytes_then_fail(
                b"ab", "drop 1",
            ))),
            Ok(ScriptedResponse::partial(
                2,
                SegReader::bytes_then_fail(b"cd", "drop 2"),
            )),
            Ok(ScriptedResponse::partial(4, SegReader::bytes(b"ef"))),
        ]);
        let sleeps = std::cell::RefCell::new(Vec::new());
        let mut hooks = FetchHooks {
                progress: |_: u64, _: Option<u64>| {},
                cancelled: || false,
                sleep: |delay| sleeps.borrow_mut().push(delay),
                on_info: |_: &str| {},
        };

        let (done, hasher) =
            stream_download_to_file(&script.opener(), TEST_URL, &target, 6, &mut hooks).unwrap();

        assert_eq!(done, 6);
        assert_eq!(std::fs::read(&target).unwrap(), b"abcdef");
        verify_download(done, hasher, 6, &sha256_hex(b"abcdef"), &dir).unwrap();
        // Both failures start a new streak (bytes arrived), so both back off
        // 2 s rather than 2 s then 4 s.
        assert_eq!(total_slept(&sleeps.borrow()), Duration::from_secs(4));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn pocket_download_gives_up_after_5_retries() {
        let dir = unique_temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("pocket-farsi-v1.onnx");
        let mut scripted: Vec<Result<ScriptedResponse, String>> = vec![Ok(
            ScriptedResponse::ok(SegReader::fail("boom")),
        )];
        for _ in 0..5 {
            scripted.push(Ok(ScriptedResponse::ok(SegReader::fail("boom"))));
        }
        let script = ScriptOpener::new(scripted);
        let sleeps = std::cell::RefCell::new(Vec::new());
        let mut hooks = FetchHooks {
                progress: |_: u64, _: Option<u64>| {},
                cancelled: || false,
                sleep: |delay| sleeps.borrow_mut().push(delay),
                on_info: |_: &str| {},
        };

        let err = match stream_download_to_file(&script.opener(), TEST_URL, &target, 11, &mut hooks)
        {
            Ok(_) => panic!("5 failures with no progress must give up"),
            Err(err) => err,
        };

        match err {
            ModelError::Download { url, message } => {
                assert_eq!(url, TEST_URL);
                assert_eq!(
                    message,
                    "Voice download failed after 5 retries at 0 of 11 bytes: boom"
                );
            }
            other => panic!("expected a Download error, got {other}"),
        }
        assert_eq!(script.opens.borrow().len(), 6);
        assert_eq!(total_slept(&sleeps.borrow()), Duration::from_secs(60));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn pocket_download_first_open_failure_is_terminal() {
        let dir = unique_temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("pocket-farsi-v1.onnx");
        let script = ScriptOpener::new(vec![Err("dns boom".to_string())]);
        let sleeps = std::cell::RefCell::new(Vec::new());
        let mut hooks = FetchHooks {
                progress: |_: u64, _: Option<u64>| {},
                cancelled: || false,
                sleep: |delay| sleeps.borrow_mut().push(delay),
                on_info: |_: &str| {},
        };

        let err = match stream_download_to_file(&script.opener(), TEST_URL, &target, 11, &mut hooks)
        {
            Ok(_) => panic!("first-open failure must not retry"),
            Err(err) => err,
        };

        match err {
            ModelError::Download { message, .. } => assert_eq!(message, "dns boom"),
            other => panic!("expected a Download error, got {other}"),
        }
        assert_eq!(*script.opens.borrow(), vec![0]);
        assert!(sleeps.borrow().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn pocket_download_http_error_is_terminal() {
        let dir = unique_temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("pocket-farsi-v1.onnx");
        let script = ScriptOpener::new(vec![Ok(ScriptedResponse {
            status: 500,
            status_message: "Internal Server Error".to_string(),
            content_range_start: None,
            body: SegReader::bytes(b""),
        })]);
        let mut hooks = FetchHooks {
                progress: |_: u64, _: Option<u64>| {},
                cancelled: || false,
                sleep: |_| {},
                on_info: |_: &str| {},
        };

        let err = match stream_download_to_file(&script.opener(), TEST_URL, &target, 11, &mut hooks)
        {
            Ok(_) => panic!("HTTP 500 must not retry"),
            Err(err) => err,
        };

        match err {
            ModelError::Download { message, .. } => assert_eq!(
                message,
                format!("Voice download failed: HTTP 500 Internal Server Error from {TEST_URL}")
            ),
            other => panic!("expected a Download error, got {other}"),
        }
        assert_eq!(*script.opens.borrow(), vec![0]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn pocket_download_wrong_content_range_is_terminal() {
        let dir = unique_temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("pocket-farsi-v1.onnx");
        let script = ScriptOpener::new(vec![
            Ok(ScriptedResponse::ok(SegReader::bytes_then_fail(
                b"hello ",
                "connection reset",
            ))),
            Ok(ScriptedResponse::partial(0, SegReader::bytes(b"hello world"))),
        ]);
        let mut hooks = FetchHooks {
                progress: |_: u64, _: Option<u64>| {},
                cancelled: || false,
                sleep: |_| {},
                on_info: |_: &str| {},
        };

        let err = match stream_download_to_file(&script.opener(), TEST_URL, &target, 11, &mut hooks)
        {
            Ok(_) => panic!("a 206 for the wrong offset must not retry"),
            Err(err) => err,
        };

        match err {
            ModelError::Download { message, .. } => assert_eq!(
                message,
                "Voice download failed: HTTP 206 with an unexpected Content-Range for resume from 6 bytes"
            ),
            other => panic!("expected a Download error, got {other}"),
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn pocket_download_cancel_during_backoff_aborts() {
        let dir = unique_temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("pocket-farsi-v1.onnx");
        let script = ScriptOpener::new(vec![Ok(ScriptedResponse::ok(
            SegReader::bytes_then_fail(b"ab", "connection reset"),
        ))]);
        let cancelled = std::cell::Cell::new(false);
        let sleeps = std::cell::RefCell::new(Vec::new());
        let mut hooks = FetchHooks {
                progress: |_: u64, _: Option<u64>| {},
                cancelled: || cancelled.get(),
                sleep: |delay| {
                    sleeps.borrow_mut().push(delay);
                    cancelled.set(true);
                },
                on_info: |_: &str| {},
        };

        let err = match stream_download_to_file(&script.opener(), TEST_URL, &target, 11, &mut hooks)
        {
            Ok(_) => panic!("cancel during back-off must abort"),
            Err(err) => err,
        };

        match err {
            ModelError::Download { message, .. } => {
                assert_eq!(message, "Voice download cancelled");
            }
            other => panic!("expected a Download error, got {other}"),
        }
        // The back-off sleeps in ≤ 250 ms slices, so one slice ran before the
        // cancel landed.
        assert_eq!(*sleeps.borrow(), vec![Duration::from_millis(250)]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn pocket_download_wrong_size_fails() {
        let dir = unique_temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let mut hasher = ring::digest::Context::new(&ring::digest::SHA256);
        hasher.update(b"short");
        let err = verify_download(done_of(b"short"), hasher, 11, &sha256_hex(b"short"), &dir)
            .expect_err("short download must fail the size check");
        match err {
            ModelError::Unpack { message, .. } => assert_eq!(
                message,
                "Voice download is incomplete: expected 11 bytes, got 5"
            ),
            other => panic!("expected an Unpack error, got {other}"),
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn pocket_download_wrong_hash_fails() {
        let dir = unique_temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let mut hasher = ring::digest::Context::new(&ring::digest::SHA256);
        hasher.update(b"hello world");
        let actual = sha256_hex(b"hello world");
        let err = verify_download(
            done_of(b"hello world"),
            hasher,
            11,
            &"0".repeat(64),
            &dir,
        )
        .expect_err("wrong hash must fail the hash check");
        match err {
            ModelError::Unpack { message, .. } => assert_eq!(
                message,
                format!("Voice download is corrupted: expected SHA-256 {}, got {actual}", "0".repeat(64))
            ),
            other => panic!("expected an Unpack error, got {other}"),
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn done_of(data: &[u8]) -> u64 {
        data.len() as u64
    }
}
