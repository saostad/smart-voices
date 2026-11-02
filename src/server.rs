//! Contract 1 HTTP API. No desktop, audio device, download or daemon setup.
use std::collections::{BTreeSet, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::engine::ModelVariant;
use crate::http_transport::{Request, Server};
use crate::lang::Lang;
use crate::models::ModelId;
use crate::settings::Settings;
use base64::Engine;
use serde::Deserialize;
use serde_json::{Value, json};
use tiny_http::{Header, Response};

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug)]
pub struct ApiError {
    status: u16,
    code: Option<u8>,
    kind: &'static str,
    message: String,
}
impl ApiError {
    fn new(status: u16, code: Option<u8>, kind: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            kind,
            message: message.into(),
        }
    }
    fn bad(message: &str) -> Self {
        Self::new(400, Some(2), "invalid_request", message)
    }
    fn internal(stage: &str, detail: &str) -> Self {
        // Engine errors can include input text. Log stage only, never raw engine output.
        if stage == "encoding" {
            eprintln!("error: HTTP encoding failed: {detail}");
        } else {
            eprintln!(
                "error: HTTP {stage} failed (input-bearing detail redacted, detail_bytes={})",
                detail.len()
            );
        }
        Self::new(
            500,
            Some(6),
            "synthesis_failed",
            "Audio generation failed; check the service logs",
        )
    }
}

#[derive(Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Fa,
    En,
    #[default]
    Auto,
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Input {
    #[default]
    Markdown,
    Plain,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Speak {
    text: String,
    #[serde(default)]
    language: Language,
    voice: Option<String>,
    speed: Option<f32>,
    #[serde(default = "default_format")]
    format: String,
    #[serde(default)]
    input: Input,
    max_chars: Option<usize>,
    #[serde(default)]
    timings: bool,
}
fn default_format() -> String {
    "ogg".into()
}

pub struct Audio {
    bytes: Vec<u8>,
    mime: &'static str,
    duration: f64,
    language: String,
    voice: String,
    model: String,
    rate: u32,
}
/// Each worker owns its synthesizer, including its lazy engine cache.
pub trait Synthesizer {
    fn speak(&mut self, request: &Speak) -> Result<Audio, ApiError>;
    fn unload(&mut self);
}
struct Native {
    settings: Settings,
    root: PathBuf,
    dictionary: crate::pronounce::Pronunciations,
    english: Option<Box<dyn crate::player::Synth>>,
    persian: Option<Box<dyn crate::player::Synth>>,
    persian_id: Option<crate::engine::PersianVoice>,
    loaded: Arc<Mutex<BTreeSet<String>>>,
    worker: usize,
}
fn missing(model: ModelId, settings: &Settings) -> ApiError {
    let command = match model {
        ModelId::Persian(voice) => format!("smart-voices download --persian-voice {}", voice.id()),
        ModelId::PhononVoices => "smart-voices download --model phonon".into(),
        _ => format!(
            "smart-voices download --model {}",
            serde_json::to_value(settings.model)
                .expect("model serialization")
                .as_str()
                .expect("model string")
        ),
    };
    ApiError::new(
        503,
        Some(3),
        "model_missing",
        format!("{} is not installed; run `{command}`", model.label()),
    )
}
fn prepare(
    request: &Speak,
    dictionary: &crate::pronounce::Pronunciations,
) -> Result<Vec<crate::chunk::SpeechChunk>, ApiError> {
    let chunks = match request.input {
        Input::Markdown => crate::pipeline::prepare(
            &request.text,
            &crate::sanitize::SanitizerOptions::default(),
            dictionary,
        ),
        Input::Plain => {
            crate::chunk::SpeechChunker::new().chunks(&[crate::sanitize::SpeechSegment {
                text: request
                    .text
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" "),
                pause: crate::sanitize::SpeechPause::None,
            }])
        }
    };
    if chunks.is_empty() {
        return Err(ApiError::new(
            422,
            Some(4),
            "empty_text",
            "No speakable text remains after cleanup",
        ));
    }
    for chunk in &chunks {
        let lang = crate::lang::chunk_language(&chunk.text);
        if (request.language == Language::En && lang == Lang::Persian)
            || (request.language == Language::Fa && lang == Lang::English)
        {
            return Err(ApiError::new(
                422,
                Some(7),
                "language_unsupported",
                "The requested language model cannot serve this text; use language auto for mixed text",
            ));
        }
    }
    Ok(chunks)
}
fn request_settings(defaults: &Settings, request: &Speak) -> Result<Settings, ApiError> {
    let mut settings = defaults.clone();
    if let Some(speed) = request.speed {
        settings.speed = speed;
    }
    if let Some(voice) = &request.voice {
        if let Some(persian) = crate::engine::PersianVoice::parse(voice) {
            settings.persian_voice = persian;
        } else {
            let key = match settings.model {
                ModelVariant::Phonon => "phonon_voice",
                ModelVariant::Paradee => "paradee_voice",
                _ => "voice",
            };
            crate::cli::set(&mut settings, key, voice)
        .map_err(|_| ApiError::bad("Unknown voice; see /v1/capabilities for valid Persian and configured English model voice ids"))?;
        }
    }
    Ok(settings)
}
impl Synthesizer for Native {
    fn speak(&mut self, request: &Speak) -> Result<Audio, ApiError> {
        let settings = request_settings(&self.settings, request)?;
        if self.persian_id != Some(settings.persian_voice) {
            self.persian = None;
            if let Some(previous) = self.persian_id {
                self.loaded.lock().expect("loaded lock").remove(&format!(
                    "{}:{}",
                    self.worker,
                    previous.dir_name()
                ));
            }
            self.persian_id = Some(settings.persian_voice);
        }
        let chunks = prepare(request, &self.dictionary)?;
        let format = crate::export::Format::parse(&request.format)
            .map_err(|_| ApiError::bad("Invalid format"))?;
        format.check_encoder().map_err(|_| {
            ApiError::new(
                415,
                Some(4),
                "encoder_unavailable",
                "Install ffmpeg with the requested audio encoder, or request wav",
            )
        })?;
        if let Some(model) = crate::export::missing_models(&self.root, &chunks, &settings).first() {
            return Err(missing(*model, &settings));
        }
        let mut languages = BTreeSet::new();
        let mut voices = BTreeSet::new();
        let mut models = BTreeSet::new();
        for chunk in &chunks {
            let lang = crate::lang::chunk_language(&chunk.text);
            languages.insert(if lang == Lang::English { "en" } else { "fa" });
            voices.insert(crate::export::voice_for_language(&settings, lang).to_owned());
            models.insert(match lang {
                Lang::English => serde_json::to_value(settings.model)
                    .expect("model")
                    .as_str()
                    .expect("string")
                    .to_owned(),
                Lang::Persian => settings.persian_voice.id().to_owned(),
            });
        }
        let audio = crate::export::render_chunks(
            &chunks,
            &settings,
            &self.dictionary,
            None,
            |lang, text, voice, speed| {
                let (slot, id) = match lang {
                    Lang::English => (&mut self.english, settings.model.dir_name()),
                    Lang::Persian => (&mut self.persian, settings.persian_voice.dir_name()),
                };
                if slot.is_none() {
                    *slot = Some(match lang {
                        Lang::English => {
                            crate::player::load_english_engine(&self.root, settings.model)?
                        }
                        Lang::Persian => {
                            crate::player::load_persian_engine(&self.root, settings.persian_voice)?
                        }
                    });
                    self.loaded
                        .lock()
                        .expect("loaded lock")
                        .insert(format!("{}:{id}", self.worker));
                }
                slot.as_mut()
                    .ok_or("engine not initialized")?
                    .synthesize(text, voice, speed)
            },
        )
        .map_err(|e| ApiError::internal("synthesis", &e))?;
        let bytes = crate::export::encode(&audio, format, false)
            .map_err(|e| ApiError::internal("encoding", &e))?;
        Ok(Audio {
            bytes,
            mime: match format {
                crate::export::Format::Wav => "audio/wav",
                crate::export::Format::Mp3 => "audio/mpeg",
                _ => "audio/ogg",
            },
            duration: audio.samples.len() as f64 / f64::from(audio.sample_rate),
            language: if languages.len() == 1 {
                languages.first().expect("language").to_string()
            } else {
                "mixed".into()
            },
            voice: voices.into_iter().collect::<Vec<_>>().join(","),
            model: models.into_iter().collect::<Vec<_>>().join(","),
            // Ogg Opus uses a fixed 48 kHz decode clock, irrespective of input rate.
            rate: if format == crate::export::Format::Opus {
                48000
            } else {
                audio.sample_rate
            },
        })
    }
    fn unload(&mut self) {
        self.english = None;
        self.persian = None;
        let prefix = format!("{}:", self.worker);
        self.loaded
            .lock()
            .expect("loaded lock")
            .retain(|id| !id.starts_with(&prefix));
    }
}

/// Hash both values to equal-sized inputs before ring's constant-time comparison.
fn authorized(expected: Option<&[u8; 32]>, header: Option<&str>) -> bool {
    let Some(expected) = expected else {
        return true;
    };
    let candidate = header.and_then(|s| s.strip_prefix("Bearer ")).unwrap_or("");
    let digest = ring::digest::digest(&ring::digest::SHA256, candidate.as_bytes());
    // ring 0.17 exposes this deprecated entry point; keep the comparison constant-time.
    #[allow(deprecated)]
    ring::constant_time::verify_slices_are_equal(expected, digest.as_ref()).is_ok()
}
fn token(path: &str) -> Result<Option<[u8; 32]>, String> {
    if path.is_empty() {
        return Ok(None);
    }
    let value = std::fs::read_to_string(path).map_err(|_| "couldn't read token file")?;
    let value = value.trim();
    if value.is_empty() {
        return Err("token file is empty".into());
    }
    Ok(Some(
        ring::digest::digest(&ring::digest::SHA256, value.as_bytes())
            .as_ref()
            .try_into()
            .expect("SHA256"),
    ))
}
struct Reply {
    status: u16,
    bytes: Vec<u8>,
    headers: Vec<(&'static str, String)>,
}
impl Reply {
    fn json(status: u16, value: Value) -> Self {
        Self {
            status,
            bytes: value.to_string().into_bytes(),
            headers: vec![("Content-Type", "application/json".into())],
        }
    }
    fn error(error: ApiError) -> Self {
        let mut reply = Self::json(
            error.status,
            json!({"contract":1,"error":{"code":error.code,"kind":error.kind,"message":error.message}}),
        );
        if reply.status == 401 {
            reply.headers.push(("WWW-Authenticate", "Bearer".into()));
        }
        if reply.status == 429 {
            reply.headers.push(("Retry-After", "1".into()));
        }
        reply
    }
}
fn audio_reply(audio: Audio, timings: bool) -> Reply {
    if timings {
        Reply::json(
            200,
            json!({"contract":1,"version":VERSION,"mimeType":audio.mime,"durationSec":audio.duration,"language":audio.language,"voice":audio.voice,"model":audio.model,"sampleRate":audio.rate,"audioBase64":base64::engine::general_purpose::STANDARD.encode(audio.bytes),"words":[],"wordsAvailable":false}),
        )
    } else {
        Reply {
            status: 200,
            bytes: audio.bytes,
            headers: vec![
                ("Content-Type", audio.mime.into()),
                ("X-Contract", "1".into()),
                ("X-Duration-Sec", audio.duration.to_string()),
                ("X-Language", audio.language),
                ("X-Voice", audio.voice),
                ("X-Model", audio.model),
                ("X-Sample-Rate", audio.rate.to_string()),
            ],
        }
    }
}
fn validate(body: &[u8], settings: &Settings) -> Result<Speak, ApiError> {
    if body.len() > settings.server.max_body_bytes {
        return Err(ApiError::new(
            413,
            Some(5),
            "body_limit",
            "Request body exceeds the service limit",
        ));
    }
    let request: Speak = serde_json::from_slice(body)
        .map_err(|_| ApiError::bad("Invalid JSON, unknown field, or wrong field type"))?;
    if !["ogg", "wav", "mp3"].contains(&request.format.as_str()) {
        return Err(ApiError::bad("format must be ogg, wav or mp3"));
    }
    if request
        .speed
        .is_some_and(|s| !s.is_finite() || !(0.5..=2.).contains(&s))
    {
        return Err(ApiError::bad("speed must be between 0.5 and 2"));
    }
    if request
        .max_chars
        .is_some_and(|n| n == 0 || n > settings.server.max_chars)
    {
        return Err(ApiError::bad(
            "maxChars must be positive and at most the service limit",
        ));
    }
    if request.text.chars().count() > request.max_chars.unwrap_or(settings.server.max_chars) {
        return Err(ApiError::new(
            413,
            Some(5),
            "text_limit",
            "Text exceeds the character limit",
        ));
    }
    if request.text.trim().is_empty() {
        return Err(ApiError::new(
            422,
            Some(4),
            "empty_text",
            "Text must not be empty",
        ));
    }
    Ok(request)
}
fn english_models(settings: &Settings, variant: ModelVariant) -> Vec<ModelId> {
    let chunks = [crate::chunk::SpeechChunk {
        text: "English.".into(),
        pause_after: crate::sanitize::SpeechPause::None,
    }];
    crate::models::models_for_chunks(
        &chunks,
        variant,
        settings.persian_voice,
        &settings.phonon_voice,
    )
}
fn default_missing(settings: &Settings, root: &Path) -> Vec<&'static str> {
    english_models(settings, settings.model)
        .into_iter()
        .chain(std::iter::once(ModelId::Persian(settings.persian_voice)))
        .filter(|model| !crate::models::is_installed(root, *model))
        .map(|model| model.label())
        .collect()
}
fn capabilities(settings: &Settings, root: &Path) -> Value {
    let mut models = Vec::new();
    let mut voices = serde_json::Map::new();
    for id in ["fast", "small", "phonon", "paradee"] {
        let variant = ModelVariant::parse(id).expect("catalog");
        let model = ModelId::English(variant);
        // Include required support data in installation readiness.
        let installed = english_models(settings, variant)
            .iter()
            .all(|m| crate::models::is_installed(root, *m));
        models
            .push(json!({"id":id,"label":model.label(),"languages":["en"],"installed":installed}));
        voices.insert(
            id.into(),
            match variant {
                ModelVariant::Phonon => json!(
                    std::iter::once("default")
                        .chain(crate::phonon::PHONON_VOICES.iter().map(|v| v.id))
                        .collect::<Vec<_>>()
                ),
                ModelVariant::Paradee => json!(crate::engine::PARADEE_VOICES),
                _ => json!(crate::engine::VOICES.to_vec()),
            },
        );
    }
    for voice in crate::engine::PERSIAN_VOICES {
        let model = ModelId::Persian(voice);
        models.push(json!({"id":voice.id(),"label":model.label(),"languages":["fa"],"installed":crate::models::is_installed(root, model)}));
        voices.insert(voice.id().into(), json!([voice.id()]));
    }
    let formats: Vec<_> = [
        ("ogg", crate::export::Format::Opus),
        ("wav", crate::export::Format::Wav),
        ("mp3", crate::export::Format::Mp3),
    ]
    .into_iter()
    .filter(|(_, f)| f.check_encoder().is_ok())
    .map(|(id, _)| id)
    .collect();
    json!({"contract":1,"version":VERSION,"models":models,"voices":voices,"formats":formats,"languages":["fa","en","auto"],"limits":{"maxChars":settings.server.max_chars,"maxBodyBytes":settings.server.max_body_bytes,"maxConcurrency":settings.server.max_concurrency,"maxQueue":settings.server.max_queue,"maxHandlers":settings.server.max_handlers}})
}
fn route(method: &str, path: &str, auth: bool) -> Result<bool, ApiError> {
    if method == "GET" && path == "/v1/health" {
        return Ok(false);
    }
    if path != "/v1/health" && !auth {
        return Err(ApiError::new(
            401,
            None,
            "unauthorized",
            "A valid bearer token is required",
        ));
    }
    let expected = match path {
        "/v1/health" | "/v1/capabilities" => "GET",
        "/v1/speak" => "POST",
        _ => return Err(ApiError::new(404, None, "not_found", "Route not found")),
    };
    if method != expected {
        return Err(ApiError::new(
            405,
            None,
            "method_not_allowed",
            "Method not allowed",
        ));
    }
    Ok(path == "/v1/speak")
}
struct Job {
    request: Request,
    started: Instant,
    speak: Speak,
}
fn respond(request: Request, reply: Reply, started: Instant, chars: usize) {
    let method = request.method().to_string();
    // Never log query strings (they may contain caller secrets).
    let path = request.url().split('?').next().unwrap_or("/");
    let path = match path {
        "/v1/health" | "/v1/capabilities" | "/v1/speak" => path,
        _ => "<unknown>",
    }
    .to_string();
    let status = reply.status;
    let mut response = Response::from_data(reply.bytes).with_status_code(status);
    for (key, value) in reply.headers {
        response.add_header(Header::from_bytes(key, value).expect("safe response header"));
    }
    if let Err(e) = request.respond(response) {
        eprintln!("error: HTTP response write failed: {e}");
    }
    eprintln!(
        "{method} {path} {status} {}ms chars={chars}",
        started.elapsed().as_millis()
    );
}
#[derive(Default)]
struct Queue {
    jobs: VecDeque<Job>,
    closed: bool,
    active: usize,
}
type SharedQueue = Arc<(Mutex<Queue>, Condvar)>;
fn worker(mut synth: Box<dyn Synthesizer + Send>, queue: SharedQueue, idle: u64) {
    let mut last_used = Instant::now();
    loop {
        let (lock, wake) = &*queue;
        let mut state = lock.lock().expect("queue lock");
        while state.jobs.is_empty() && !state.closed {
            if idle == 0 {
                state = wake.wait(state).expect("queue wait");
            } else {
                let remaining = Duration::from_secs(idle).saturating_sub(last_used.elapsed());
                let (next, timeout) = wake.wait_timeout(state, remaining).expect("queue wait");
                state = next;
                if timeout.timed_out() {
                    synth.unload();
                    last_used = Instant::now();
                }
            }
        }
        let Some(job) = state.jobs.pop_front() else {
            break;
        };
        state.active += 1;
        drop(state);
        let chars = job.speak.text.chars().count();
        let reply = match synth.speak(&job.speak) {
            Ok(audio) => audio_reply(audio, job.speak.timings),
            Err(error) => Reply::error(error),
        };
        respond(job.request, reply, job.started, chars);
        queue.0.lock().expect("queue lock").active -= 1;
        last_used = Instant::now();
    }
}
pub fn run(args: &[String], path: &Path) -> Result<(), String> {
    let mut settings = crate::cli::load(path)?;
    if !args.len().is_multiple_of(2) {
        return Err("serve flags require values".into());
    }
    for pair in args.as_chunks::<2>().0 {
        let key = match pair[0].as_str() {
            "--listen" => "listen",
            "--token-file" => "token_file",
            "--max-concurrency" => "max_concurrency",
            "--max-queue" => "max_queue",
            "--max-handlers" => "max_handlers",
            "--read-timeout-secs" => "read_timeout_secs",
            "--max-header-bytes" => "max_header_bytes",
            "--accept-poll-ms" => "accept_poll_ms",
            "--max-chars" => "max_chars",
            "--max-body-bytes" => "max_body_bytes",
            "--idle-unload-secs" => "idle_unload_secs",
            _ => return Err("unknown serve flag; see --help".into()),
        };
        crate::cli::set(&mut settings, &format!("server.{key}"), &pair[1])?;
    }
    settings.validate_strict()?;
    let token = token(&settings.server.token_file)?;
    let root = crate::models::models_root().ok_or("couldn't resolve models directory")?;
    let dictionary = crate::cli::pronunciations(&crate::cli::pronunciation_path(path)?)?;
    let server =
        Server::http(&settings.server.listen).map_err(|e| format!("HTTP listen failed: {e}"))?;
    let stop = Arc::new(AtomicBool::new(false));
    let signal = stop.clone();
    ctrlc::set_handler(move || signal.store(true, Ordering::Relaxed))
        .map_err(|e| format!("signal handler failed: {e}"))?;
    eprintln!(
        "HTTP listening={} auth={} concurrency={} queue={} max_chars={} max_body_bytes={} idle_unload_secs={} max_handlers={}",
        server.server_addr(),
        if token.is_some() { "token" } else { "none" },
        settings.server.max_concurrency,
        settings.server.max_queue,
        settings.server.max_chars,
        settings.server.max_body_bytes,
        settings.server.idle_unload_secs,
        settings.server.max_handlers
    );
    let loaded = Arc::new(Mutex::new(BTreeSet::new()));
    let catalog_settings = settings.clone();
    let catalog_root = root.clone();
    let caps = move |health| {
        if health {
            json!(default_missing(&catalog_settings, &catalog_root))
        } else {
            capabilities(&catalog_settings, &catalog_root)
        }
    };
    let worker_settings = settings.clone();
    let worker_loaded = loaded.clone();
    serve_http(server, settings, token, stop, loaded, caps, move |worker| {
        Box::new(Native {
            settings: worker_settings.clone(),
            root: root.clone(),
            dictionary: dictionary.clone(),
            english: None,
            persian: None,
            persian_id: None,
            loaded: worker_loaded.clone(),
            worker,
        })
    })
}

/// Shared transport loop; tests inject a device-free fake without encoders or models.
fn serve_http(
    server: Server,
    settings: Settings,
    token: Option<[u8; 32]>,
    stop: Arc<AtomicBool>,
    loaded: Arc<Mutex<BTreeSet<String>>>,
    caps: impl Fn(bool) -> Value + Send + Sync + 'static,
    mut factory: impl FnMut(usize) -> Box<dyn Synthesizer + Send>,
) -> Result<(), String> {
    let queue: SharedQueue = Arc::default();
    let mut workers = Vec::new();
    for worker_id in 0..settings.server.max_concurrency {
        let native = factory(worker_id);
        let queue = queue.clone();
        let idle = settings.server.idle_unload_secs;
        workers.push(std::thread::spawn(move || worker(native, queue, idle)));
    }
    let caps = Arc::new(caps);
    let settings = Arc::new(settings);
    let handlers = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    while !stop.load(Ordering::Relaxed) {
        let Some(request) = server
            .recv_timeout(Duration::from_millis(settings.server.accept_poll_ms))
            .map_err(|e| format!("HTTP receive failed: {e}"))?
        else {
            continue;
        };
        if handlers.load(Ordering::Acquire) >= settings.server.max_handlers {
            respond(
                request,
                Reply::error(ApiError::new(
                    429,
                    None,
                    "queue_full",
                    "Service is busy; retry later",
                )),
                Instant::now(),
                0,
            );
            continue;
        }
        handlers.fetch_add(1, Ordering::AcqRel);
        let handlers = handlers.clone();
        let settings = settings.clone();
        let queue = queue.clone();
        let loaded = loaded.clone();
        let caps = caps.clone();
        std::thread::spawn(move || {
            struct Handler(Arc<std::sync::atomic::AtomicUsize>);
            impl Drop for Handler {
                fn drop(&mut self) {
                    self.0.fetch_sub(1, Ordering::AcqRel);
                }
            }
            let _handler = Handler(handlers);
            let mut request = request;
            let started = Instant::now();
            if let Err(error) = request.read_headers(
                Duration::from_secs(settings.server.read_timeout_secs),
                settings.server.max_header_bytes,
            ) {
                eprintln!("error: HTTP request headers failed: {error}");
                respond(
                    request,
                    Reply::error(ApiError::bad(
                        "Could not read request headers; use Content-Length and check service limits",
                    )),
                    started,
                    0,
                );
                return;
            }
            let path = request.url().split('?').next().unwrap_or("/");
            let auth_header = request
                .headers()
                .iter()
                .find(|h| h.field.equiv("Authorization"))
                .map(|h| h.value.as_str());
            match route(
                request.method().as_str(),
                path,
                authorized(token.as_ref(), auth_header),
            ) {
                Err(error) => respond(request, Reply::error(error), started, 0),
                Ok(false) => {
                    let reply = if path == "/v1/health" {
                        let ids: BTreeSet<String> = loaded
                            .lock()
                            .expect("loaded lock")
                            .iter()
                            .filter_map(|s| s.split_once(':').map(|(_, id)| id.to_owned()))
                            .collect();
                        let missing = caps(true);
                        Reply::json(
                            200,
                            json!({"contract":1,"version":VERSION,"ready":missing.as_array().is_some_and(|m| m.is_empty()),"missing":missing,"auth":if token.is_some() {"token"} else {"none"},"modelsLoaded":ids}),
                        )
                    } else {
                        Reply::json(200, caps(false))
                    };
                    respond(request, reply, started, 0);
                }
                Ok(true) => {
                    let content_type = request
                        .headers()
                        .iter()
                        .find(|h| h.field.equiv("Content-Type"))
                        .map(|h| h.value.as_str().split(';').next().unwrap_or("").trim());
                    if !content_type.is_some_and(|s| s.eq_ignore_ascii_case("application/json")) {
                        respond(
                            request,
                            Reply::error(ApiError::new(
                                415,
                                Some(4),
                                "content_type",
                                "Use Content-Type: application/json",
                            )),
                            started,
                            0,
                        );
                        return;
                    }
                    if request
                        .body_length()
                        .is_some_and(|n| n > settings.server.max_body_bytes)
                    {
                        respond(
                            request,
                            Reply::error(ApiError::new(
                                413,
                                Some(5),
                                "body_limit",
                                "Request body exceeds the service limit",
                            )),
                            started,
                            0,
                        );
                        return;
                    }
                    let mut body = Vec::new();
                    if let Err(e) = request
                        .as_reader()
                        .take(settings.server.max_body_bytes as u64 + 1)
                        .read_to_end(&mut body)
                    {
                        eprintln!("error: HTTP body read failed: {e}");
                        respond(
                            request,
                            Reply::error(ApiError::bad("Could not read request body")),
                            started,
                            0,
                        );
                        return;
                    }
                    match validate(&body, &settings) {
                        Err(error) => respond(request, Reply::error(error), started, 0),
                        Ok(speak) => {
                            let job = Job {
                                request,
                                started,
                                speak,
                            };
                            let mut state = queue.0.lock().expect("queue lock");
                            if state.closed {
                                drop(state);
                                respond(
                                    job.request,
                                    Reply::error(ApiError::new(
                                        503,
                                        None,
                                        "shutting_down",
                                        "Service is shutting down; retry later",
                                    )),
                                    started,
                                    0,
                                );
                            } else if state.jobs.len() + state.active
                                >= settings.server.max_queue + settings.server.max_concurrency
                            {
                                drop(state);
                                respond(
                                    job.request,
                                    Reply::error(ApiError::new(
                                        429,
                                        None,
                                        "queue_full",
                                        "Service is busy; retry later",
                                    )),
                                    started,
                                    job.speak.text.chars().count(),
                                );
                            } else {
                                state.jobs.push_back(job);
                                queue.1.notify_one();
                            }
                        }
                    }
                }
            }
        });
    }
    queue.0.lock().expect("queue lock").closed = true;
    queue.1.notify_all();
    for worker in workers {
        worker.join().map_err(|_| "HTTP worker panicked")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpStream;
    use std::sync::mpsc;

    struct Fake;
    impl Synthesizer for Fake {
        fn speak(&mut self, request: &Speak) -> Result<Audio, ApiError> {
            prepare(request, &crate::pronounce::Pronunciations::new(vec![]))?;
            if request.text == "missing" {
                return Err(missing(
                    ModelId::English(ModelVariant::Fast),
                    &Settings::default(),
                ));
            }
            Ok(Audio {
                bytes: b"audio".to_vec(),
                mime: "audio/wav",
                duration: 1.5,
                language: "en".into(),
                voice: "af_heart".into(),
                model: "fast".into(),
                rate: 24000,
            })
        }
        fn unload(&mut self) {}
    }
    struct TestServer {
        address: String,
        stop: Arc<AtomicBool>,
        thread: Option<std::thread::JoinHandle<Result<(), String>>>,
    }
    impl TestServer {
        fn start(
            settings: Settings,
            token: Option<[u8; 32]>,
            factory: impl FnMut(usize) -> Box<dyn Synthesizer + Send> + Send + 'static,
        ) -> Self {
            let server = Server::http("127.0.0.1:0").unwrap();
            let address = server.server_addr().to_string();
            let stop = Arc::new(AtomicBool::new(false));
            let signal = stop.clone();
            let thread = std::thread::spawn(move || {
                serve_http(
                    server,
                    settings,
                    token,
                    signal,
                    Arc::default(),
                    |health| {
                        if health {
                            json!([])
                        } else {
                            json!({"contract":1})
                        }
                    },
                    factory,
                )
            });
            Self {
                address,
                stop,
                thread: Some(thread),
            }
        }
        fn connect(&self, method: &str, path: &str, body: &str, extra: &str) -> TcpStream {
            let mut stream = TcpStream::connect(&self.address).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            write!(
                stream,
                "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{extra}\r\n{body}",
                body.len()
            )
            .unwrap();
            stream
        }
        fn request(&self, method: &str, path: &str, body: &str, extra: &str) -> String {
            let mut result = String::new();
            self.connect(method, path, body, extra)
                .read_to_string(&mut result)
                .unwrap();
            result
        }
    }
    impl Drop for TestServer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            self.thread.take().unwrap().join().unwrap().unwrap();
        }
    }
    fn digest(s: &str) -> [u8; 32] {
        ring::digest::digest(&ring::digest::SHA256, s.as_bytes())
            .as_ref()
            .try_into()
            .unwrap()
    }
    #[test]
    fn stalled_body_does_not_block_health_or_capabilities() {
        let server = TestServer::start(Settings::default(), None, |_| Box::new(Fake));
        // A body larger than tiny_http's old eager buffer also exercises the original bug.
        let mut stalled = TcpStream::connect(&server.address).unwrap();
        write!(stalled, "POST /v1/speak HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 4096\r\nExpect: 100-continue\r\n\r\n{{").unwrap();
        stalled
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut interim = Vec::new();
        while !interim.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            stalled.read_exact(&mut byte).unwrap();
            interim.push(byte[0]);
        }
        assert!(interim.starts_with(b"HTTP/1.1 100 Continue\r\n"));
        // The interim response proves our handler has reached the stalled read.
        for path in ["/v1/health", "/v1/capabilities", "/v1/health"] {
            let mut health = server.connect("GET", path, "", "");
            health
                .set_read_timeout(Some(Duration::from_millis(500)))
                .unwrap();
            let mut response = String::new();
            let result = health.read_to_string(&mut response);
            // Release the stalled upload even when checking the old implementation.
            if result.is_err() {
                stalled.shutdown(std::net::Shutdown::Both).unwrap();
            }
            assert!(
                result.is_ok(),
                "{path} blocked behind incomplete body: {result:?}"
            );
            assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        }
        stalled.shutdown(std::net::Shutdown::Both).unwrap();
    }
    #[test]
    fn handler_overflow_rejects_incomplete_body_without_blocking_accept() {
        let mut settings = Settings::default();
        settings.server.max_handlers = 1;
        let server = TestServer::start(settings, None, |_| Box::new(Fake));
        let mut stalled = TcpStream::connect(&server.address).unwrap();
        stalled
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        write!(stalled, "POST /v1/speak HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 4096\r\nExpect: 100-continue\r\n\r\n{{").unwrap();
        let mut interim = [0; 25];
        stalled.read_exact(&mut interim).unwrap();
        let mut overflow = TcpStream::connect(&server.address).unwrap();
        overflow
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        write!(overflow, "POST /v1/speak HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 4096\r\n\r\n{{").unwrap();
        fn read_response(stream: &mut TcpStream) -> String {
            // HTTP clients read Content-Length, not EOF (an unread upload may
            // cause a TCP reset after the complete rejection response).
            let mut headers = Vec::new();
            while !headers.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                headers.push(byte[0]);
            }
            let headers = String::from_utf8(headers).unwrap();
            let length: usize = headers
                .lines()
                .find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.eq_ignore_ascii_case("Content-Length")
                        .then(|| value.trim().parse().unwrap())
                })
                .unwrap();
            let mut body = vec![0; length];
            stream.read_exact(&mut body).unwrap();
            let body = String::from_utf8(body).unwrap();
            serde_json::from_str::<Value>(&body).unwrap();
            headers + &body
        }
        let reply = read_response(&mut overflow);
        assert!(reply.starts_with("HTTP/1.1 429"), "{reply}");
        assert!(reply.contains("\"kind\":\"queue_full\""));
        // Keep the rejected socket open too. At capacity, health must promptly
        // receive 429 rather than hang waiting for either incomplete upload.
        let mut health = server.connect("GET", "/v1/health", "", "");
        health
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        let reply = read_response(&mut health);
        assert!(reply.starts_with("HTTP/1.1 429"), "{reply}");
        stalled.shutdown(std::net::Shutdown::Both).unwrap();
    }
    #[test]
    fn request_voice_overrides_are_typed_and_never_persisted() {
        let defaults = Settings::default();
        for voice in crate::engine::PERSIAN_VOICES {
            let request = validate(
                json!({"text":"سلام", "language":"fa", "voice":voice.id()})
                    .to_string()
                    .as_bytes(),
                &defaults,
            )
            .unwrap();
            let settings = request_settings(&defaults, &request).unwrap();
            assert_eq!(settings.persian_voice, voice);
            assert_eq!(settings.voice, defaults.voice);
        }
        let request = validate(br#"{"text":"Hello", "voice":"af_bella"}"#, &defaults).unwrap();
        let settings = request_settings(&defaults, &request).unwrap();
        assert_eq!(settings.voice, "af_bella");
        assert_eq!(settings.persian_voice, defaults.persian_voice);
        let request = validate(br#"{"text":"Hello", "voice":"unknown"}"#, &defaults).unwrap();
        let error = request_settings(&defaults, &request).unwrap_err();
        assert_eq!((error.status, error.code), (400, Some(2)));
        assert!(error.message.contains("/v1/capabilities"));
        assert_eq!(defaults, Settings::default());
    }
    #[test]
    fn health_missing_models_include_default_engines_and_support_data() {
        let root = std::env::temp_dir().join(format!(
            "smart-voices-health-no-models-{}",
            std::process::id()
        ));
        assert!(!root.exists());
        let settings = Settings::default();
        let missing = default_missing(&settings, &root);
        for model in english_models(&settings, settings.model)
            .into_iter()
            .chain(std::iter::once(ModelId::Persian(settings.persian_voice)))
        {
            assert!(missing.contains(&model.label()));
        }
        let caps = capabilities(&settings, &root);
        assert!(
            caps["models"]
                .as_array()
                .unwrap()
                .iter()
                .all(|m| m["installed"] == false)
        );
    }
    #[test]
    fn token_comparison_handles_prefix_length_and_absence() {
        let hash = digest("secret");
        assert!(authorized(None, None));
        assert!(authorized(Some(&hash), Some("Bearer secret")));
        for value in [
            None,
            Some("secret"),
            Some("bearer secret"),
            Some("Bearer secre"),
            Some("Bearer secretX"),
            Some("Bearer xxxxxx"),
        ] {
            assert!(!authorized(Some(&hash), value));
        }
    }
    #[test]
    fn routing_auth_errors_and_audio_contract_over_http() {
        let settings = Settings {
            server: crate::settings::ServerSettings {
                max_chars: 20,
                max_body_bytes: 200,
                ..Default::default()
            },
            ..Default::default()
        };
        let server = TestServer::start(settings, Some(digest("secret")), |_| Box::new(Fake));
        assert!(
            server
                .request("GET", "/v1/health", "", "")
                .contains("\"auth\":\"token\"")
        );
        let denied = server.request("GET", "/v1/capabilities", "", "");
        assert!(denied.starts_with("HTTP/1.1 401"));
        assert!(denied.to_lowercase().contains("www-authenticate: bearer"));
        let auth = "Authorization: Bearer secret\r\n";
        for (method, path, status) in [
            ("GET", "/missing", 404),
            ("GET", "/v1/speak", 405),
            ("POST", "/v1/health", 405),
        ] {
            assert!(
                server
                    .request(method, path, "", auth)
                    .starts_with(&format!("HTTP/1.1 {status}"))
            );
        }
        for (body, status, code) in [
            ("{", 400, 2),
            (r#"{"text":"hi","unknown":true}"#, 400, 2),
            (r#"{"text":" "}"#, 422, 4),
            (r#"{"text":"** **"}"#, 422, 4),
            (r#"{"text":"missing"}"#, 503, 3),
            (r#"{"text":"123456789012345678901"}"#, 413, 5),
            (r#"{"text":"hi","maxChars":1}"#, 413, 5),
            (r#"{"text":"hi","speed":3}"#, 400, 2),
            (r#"{"text":"hi","language":"fr"}"#, 400, 2),
            (r#"{"text":"سلام","language":"en"}"#, 422, 7),
        ] {
            let response = server.request("POST", "/v1/speak", body, auth);
            assert!(
                response.starts_with(&format!("HTTP/1.1 {status}")),
                "{response}"
            );
            assert!(response.contains(&format!("\"code\":{code}")), "{response}");
            if status == 503 {
                assert!(response.contains("smart-voices download --model fast"));
            }
        }
        assert!(
            server
                .request("POST", "/v1/speak", &"x".repeat(201), auth)
                .starts_with("HTTP/1.1 413")
        );
        let audio = server.request(
            "POST",
            "/v1/speak",
            r#"{"text":"hello","format":"wav"}"#,
            auth,
        );
        for header in [
            "content-type: audio/wav",
            "x-contract: 1",
            "x-duration-sec: 1.5",
            "x-language: en",
            "x-voice: af_heart",
            "x-model: fast",
            "x-sample-rate: 24000",
        ] {
            assert!(audio.to_lowercase().contains(header), "{audio}");
        }
        let timings = server.request("POST", "/v1/speak", r#"{"text":"hi","timings":true}"#, auth);
        let value: Value = serde_json::from_str(timings.split("\r\n\r\n").last().unwrap()).unwrap();
        assert_eq!(value["wordsAvailable"], false);
        assert_eq!(value["words"], json!([]));
        assert_eq!(value["sampleRate"], 24000);
        assert_eq!(
            value["audioBase64"],
            base64::engine::general_purpose::STANDARD.encode(b"audio")
        );
    }
    #[test]
    fn unauthenticated_service_accepts_requests() {
        let server = TestServer::start(Settings::default(), None, |_| Box::new(Fake));
        assert!(
            server
                .request("GET", "/v1/capabilities", "", "")
                .starts_with("HTTP/1.1 200")
        );
        assert!(
            server
                .request("POST", "/v1/speak", r#"{"text":"hi"}"#, "")
                .starts_with("HTTP/1.1 200")
        );
    }
    #[test]
    fn queue_full_returns_retry_after_without_waiting_for_synthesis() {
        struct Blocked {
            entered: mpsc::Sender<()>,
            release: mpsc::Receiver<()>,
        }
        impl Synthesizer for Blocked {
            fn speak(&mut self, request: &Speak) -> Result<Audio, ApiError> {
                self.entered.send(()).unwrap();
                self.release.recv().unwrap();
                Fake.speak(request)
            }
            fn unload(&mut self) {}
        }
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let mut fake = Some(Blocked {
            entered: entered_tx,
            release: release_rx,
        });
        let settings = Settings {
            server: crate::settings::ServerSettings {
                max_queue: 1,
                ..Default::default()
            },
            ..Default::default()
        };
        let server = TestServer::start(settings, None, move |_| Box::new(fake.take().unwrap()));
        // Release blocked jobs even if an assertion fails before normal teardown.
        struct ReleaseOnDrop(mpsc::Sender<()>);
        impl Drop for ReleaseOnDrop {
            fn drop(&mut self) {
                for _ in 0..2 {
                    if self.0.send(()).is_err() {
                        break;
                    }
                }
            }
        }
        let _release_on_drop = ReleaseOnDrop(release_tx.clone());

        let mut first = server.connect("POST", "/v1/speak", r#"{"text":"hi"}"#, "");
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let mut second = server.connect("POST", "/v1/speak", r#"{"text":"hi"}"#, "");
        // The health request is processed after the second request was enqueued.
        assert!(
            server
                .request("GET", "/v1/health", "", "")
                .starts_with("HTTP/1.1 200")
        );
        let full = server.request("POST", "/v1/speak", r#"{"text":"hi"}"#, "");
        release_tx.send(()).unwrap();
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        release_tx.send(()).unwrap();
        let mut out = String::new();
        first.read_to_string(&mut out).unwrap();
        second.read_to_string(&mut out).unwrap();
        assert!(full.starts_with("HTTP/1.1 429"), "{full}");
        assert!(full.to_lowercase().contains("retry-after: 1"));
    }
    #[test]
    fn native_preflight_never_downloads_missing_models() {
        let root =
            std::env::temp_dir().join(format!("smart-voices-no-models-{}", std::process::id()));
        assert!(!root.exists());
        let mut native = Native {
            settings: Settings::default(),
            root: root.clone(),
            dictionary: crate::pronounce::Pronunciations::new(vec![]),
            english: None,
            persian: None,
            persian_id: None,
            loaded: Arc::default(),
            worker: 0,
        };
        for (text, hint) in [
            ("Hello.", "smart-voices download --model fast"),
            (
                "سلام دنیا.",
                "smart-voices download --persian-voice pocket-v2",
            ),
        ] {
            let request = validate(
                json!({"text":text,"format":"wav"}).to_string().as_bytes(),
                &Settings::default(),
            )
            .unwrap();
            let error = match native.speak(&request) {
                Ok(_) => panic!("unexpected audio"),
                Err(error) => error,
            };
            assert_eq!(error.status, 503);
            assert_eq!(error.code, Some(3));
            assert!(error.message.contains(hint));
        }
        assert!(!root.exists());
        assert!(native.english.is_none() && native.persian.is_none());
    }
    #[test]
    fn server_settings_keys_validate_and_round_trip() {
        let mut settings = Settings::default();
        for (key, value) in [
            ("listen", "0.0.0.0:0"),
            ("token_file", "/tmp/token"),
            ("max_concurrency", "2"),
            ("max_queue", "0"),
            ("max_handlers", "2"),
            ("read_timeout_secs", "2"),
            ("max_header_bytes", "4096"),
            ("accept_poll_ms", "1"),
            ("max_chars", "20000"),
            ("max_body_bytes", "1024"),
            ("idle_unload_secs", "0"),
        ] {
            crate::cli::set(&mut settings, &format!("server.{key}"), value).unwrap();
        }
        for (key, value) in [
            ("listen", "bad"),
            ("max_concurrency", "0"),
            ("max_handlers", "0"),
            ("read_timeout_secs", "0"),
            ("read_timeout_secs", "4294967296"),
            ("max_header_bytes", "0"),
            ("accept_poll_ms", "0"),
            ("max_chars", "0"),
            ("max_body_bytes", "0"),
            ("max_queue", "-1"),
            ("idle_unload_secs", "-1"),
        ] {
            assert!(crate::cli::set(&mut settings, &format!("server.{key}"), value).is_err());
        }
        let decoded: Settings = toml::from_str(&toml::to_string(&settings).unwrap()).unwrap();
        assert_eq!(decoded, settings);
    }
}
