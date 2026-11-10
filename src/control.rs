//! Remote control of a reading: one interface for the tray app and `serve`, so
//! the CLI and HTTP API do what the tray menu does, on whichever host is running.
//!
//! Each host publishes a discovery file (its address and token, readable only
//! by this user) next to the settings file. CLI commands read it and call the
//! HTTP API; there is no second protocol.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use serde_json::{Value, json};

use crate::models::ModelId;
use crate::player::{Output, Player, PlayerEvent, PlayerState, Synth};
use crate::settings::Settings;

/// What a client can ask a running host to do (the tray menu's actions).
#[derive(Debug, Clone, PartialEq)]
pub enum ControlRequest {
    Play(String),
    ReadClipboard,
    Pause,
    Resume,
    /// Read / Pause: resume, pause, or read the clipboard when idle.
    Toggle,
    Stop,
    Next,
    Previous,
    Jump(usize),
    Status,
    ShowPanel,
    Quit,
    /// Settings already validated and saved; make them take effect.
    ApplySettings(Box<Settings>),
}

/// A request a host couldn't carry out, with a message safe to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlError {
    pub status: u16,
    pub kind: &'static str,
    pub message: String,
}

impl ControlError {
    #[must_use]
    pub fn new(status: u16, kind: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            kind,
            message: message.into(),
        }
    }
}

/// A host that owns playback: the tray app or `serve`. Every reply is the
/// playback status after the request.
pub trait ControlHost: Send + Sync {
    fn call(&self, request: ControlRequest) -> Result<Value, ControlError>;
    /// `"tray"` or `"serve"`.
    fn host(&self) -> &'static str;
}

/// What a reading is doing, as `GET /v1/playback` and `status` report it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlaybackStatus {
    pub state: &'static str,
    pub status: String,
    pub index: Option<usize>,
    pub text: Option<String>,
    pub chunks: Vec<String>,
    pub error: Option<String>,
}

impl PlaybackStatus {
    #[must_use]
    pub fn to_json(&self, host: &str) -> Value {
        json!({
            "contract": 1,
            "host": host,
            "state": if self.state.is_empty() { "idle" } else { self.state },
            "status": self.status,
            "index": self.index,
            "count": self.chunks.len(),
            "text": self.text,
            "chunks": self.chunks,
            "error": self.error,
        })
    }

    /// Fold one player event into the status.
    pub fn apply(&mut self, event: &PlayerEvent) {
        match event {
            PlayerEvent::State(state) => {
                self.state = match state {
                    PlayerState::Idle => "idle",
                    PlayerState::Loading => "loading",
                    PlayerState::Playing => "playing",
                    PlayerState::Paused => "paused",
                };
                self.status = match state {
                    PlayerState::Idle => "Ready".to_string(),
                    PlayerState::Loading => "Loading voice…".to_string(),
                    PlayerState::Paused => "Paused".to_string(),
                    PlayerState::Playing => self.reading_status(),
                };
                if *state == PlayerState::Idle {
                    self.index = None;
                    self.text = None;
                }
            }
            PlayerEvent::Playlist(chunks) => {
                self.chunks.clone_from(chunks);
                self.index = None;
                self.text = None;
                self.error = None;
            }
            PlayerEvent::Chunk {
                index, count, text, ..
            } => {
                self.index = Some(*index);
                self.text = Some(text.clone());
                if self.chunks.len() != *count {
                    self.chunks.resize(*count, String::new());
                }
                if self.state == "playing" {
                    self.status = self.reading_status();
                }
            }
            PlayerEvent::Finished => {
                self.state = "idle";
                self.status = "Ready".to_string();
                self.index = None;
                self.text = None;
            }
            PlayerEvent::Error(message) => {
                self.state = "idle";
                self.status.clone_from(message);
                self.error = Some(message.clone());
                self.index = None;
                self.text = None;
            }
            PlayerEvent::Latency { .. } => {}
        }
    }

    fn reading_status(&self) -> String {
        match self.index {
            Some(index) => format!("Reading {} of {}", index + 1, self.chunks.len()),
            None => "Preparing…".to_string(),
        }
    }
}

type PlayerSpawn =
    dyn Fn(&Settings) -> Result<(Player, mpsc::Receiver<PlayerEvent>), String> + Send + Sync;

/// Playback for `serve`, which has no window: opens the audio device and loads
/// a voice on the first request that needs them.
pub struct HeadlessControl {
    settings: Arc<RwLock<Settings>>,
    settings_path: PathBuf,
    root: PathBuf,
    spawn: Box<PlayerSpawn>,
    player: Mutex<Option<(Player, u64)>>,
    status: Arc<Mutex<PlaybackStatus>>,
    generation: Arc<AtomicU64>,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl HeadlessControl {
    #[must_use]
    pub fn new(
        settings: Arc<RwLock<Settings>>,
        settings_path: PathBuf,
        root: PathBuf,
        stop: Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        let spawn_root = root.clone();
        let spawn_path = settings_path.clone();
        let spawn = move |settings: &Settings| {
            let sink = crate::audio::AudioSink::new().map_err(|err| err.to_string())?;
            player_for(settings, &spawn_root, &spawn_path, sink)
        };
        Self::with_spawn(settings, settings_path, root, stop, Box::new(spawn))
    }

    fn with_spawn(
        settings: Arc<RwLock<Settings>>,
        settings_path: PathBuf,
        root: PathBuf,
        stop: Arc<std::sync::atomic::AtomicBool>,
        spawn: Box<PlayerSpawn>,
    ) -> Self {
        Self {
            settings,
            settings_path,
            root,
            spawn,
            player: Mutex::new(None),
            status: Arc::default(),
            generation: Arc::default(),
            stop,
        }
    }

    fn snapshot(&self) -> Value {
        self.status
            .lock()
            .expect("status lock")
            .to_json(self.host())
    }

    fn current_settings(&self) -> Settings {
        self.settings.read().expect("settings lock").clone()
    }

    /// Check the voices this text needs; requests never download models.
    fn check_models(&self, text: &str, settings: &Settings) -> Result<(), ControlError> {
        let dictionary = crate::cli::pronunciations(
            &crate::cli::pronunciation_path(&self.settings_path)
                .map_err(|message| ControlError::new(500, "pronunciations", message))?,
        )
        .map_err(|message| ControlError::new(500, "pronunciations", message))?;
        let chunks = crate::pipeline::prepare(
            text,
            &crate::sanitize::SanitizerOptions::default(),
            &dictionary,
        );
        if chunks.is_empty() {
            return Err(ControlError::new(
                422,
                "empty_text",
                "No speakable text remains after cleanup",
            ));
        }
        if let Some(model) = crate::export::missing_models(&self.root, &chunks, settings).first() {
            return Err(ControlError::new(
                503,
                "model_missing",
                format!(
                    "{} is not installed; download it with POST /v1/downloads or `smart-voices download`",
                    model.label()
                ),
            ));
        }
        Ok(())
    }

    fn play(&self, text: String) -> Result<Value, ControlError> {
        let settings = self.current_settings();
        self.check_models(&text, &settings)?;
        let mut slot = self.player.lock().expect("player lock");
        if slot.is_none() {
            let (player, events) = (self.spawn)(&settings)
                .map_err(|message| ControlError::new(503, "playback_unavailable", message))?;
            let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
            let status = self.status.clone();
            let current = self.generation.clone();
            std::thread::spawn(move || {
                for event in events {
                    if current.load(Ordering::SeqCst) != generation {
                        break;
                    }
                    status.lock().expect("status lock").apply(&event);
                }
            });
            *slot = Some((player, generation));
        }
        {
            let mut status = self.status.lock().expect("status lock");
            status.state = "loading";
            status.status = "Preparing…".to_string();
            status.error = None;
        }
        if let Some((player, _)) = slot.as_ref() {
            player.play(text);
        }
        drop(slot);
        Ok(self.snapshot())
    }

    fn with_player(&self, action: impl FnOnce(&Player)) -> Value {
        if let Some((player, _)) = self.player.lock().expect("player lock").as_ref() {
            action(player);
        }
        self.snapshot()
    }

    fn apply_settings(&self, new: &Settings) {
        let mut slot = self.player.lock().expect("player lock");
        let old = self.current_settings();
        if old.model != new.model || old.persian_voice != new.persian_voice {
            // The next reading loads the new voices, as the tray menu does.
            if let Some((player, _)) = slot.take() {
                player.stop();
            }
            self.generation.fetch_add(1, Ordering::SeqCst);
            let mut status = self.status.lock().expect("status lock");
            status.state = "idle";
            status.status = "Ready".to_string();
            status.index = None;
            status.text = None;
        } else if let Some((player, _)) = slot.as_ref() {
            player.set_speed(new.speed);
            player.set_voice(crate::export::voice_for_language(
                new,
                crate::lang::Lang::English,
            ));
            player.set_normalize(new.normalize_options());
            player.set_unload_after(unload_after(new));
        }
        drop(slot);
        *self.settings.write().expect("settings lock") = new.clone();
    }
}

fn unload_after(settings: &Settings) -> Option<Duration> {
    (settings.unload_after_minutes != 0)
        .then(|| Duration::from_secs(u64::from(settings.unload_after_minutes) * 60))
}

/// A player for `settings` on `output`.
fn player_for(
    settings: &Settings,
    root: &Path,
    settings_path: &Path,
    output: impl Output + 'static,
) -> Result<(Player, mpsc::Receiver<PlayerEvent>), String> {
    let pronunciations =
        crate::cli::pronunciations(&crate::cli::pronunciation_path(settings_path)?)?;
    let config = crate::player::PlayerConfig {
        voice: crate::export::voice_for_language(settings, crate::lang::Lang::English).to_string(),
        speed: settings.speed,
        unload_after: unload_after(settings),
        sanitizer_options: crate::sanitize::SanitizerOptions::default(),
        pronunciations,
        normalize: settings.normalize_options(),
        persian_voice: settings.persian_voice,
        debug_latency: false,
    };
    let (english_root, model) = (root.to_path_buf(), settings.model);
    let (persian_root, persian) = (root.to_path_buf(), settings.persian_voice);
    let factory = move || -> Result<Box<dyn Synth>, String> {
        crate::player::load_english_engine(&english_root, model)
    };
    let persian_factory = move || -> Result<Box<dyn Synth>, String> {
        crate::player::load_persian_engine(&persian_root, persian)
    };
    Ok(Player::spawn(
        config,
        Box::new(factory),
        Box::new(persian_factory),
        output,
    ))
}

impl ControlHost for HeadlessControl {
    fn host(&self) -> &'static str {
        "serve"
    }

    fn call(&self, request: ControlRequest) -> Result<Value, ControlError> {
        match request {
            ControlRequest::Play(text) => self.play(text),
            ControlRequest::ReadClipboard => match crate::clipboard::read_text() {
                Ok(Some(text)) if !text.trim().is_empty() => self.play(text),
                Ok(_) => Err(ControlError::new(
                    422,
                    "empty_clipboard",
                    "Nothing to read: the clipboard has no text.",
                )),
                Err(error) => Err(ControlError::new(
                    503,
                    "clipboard_unavailable",
                    format!("Couldn't read the clipboard: {error}"),
                )),
            },
            ControlRequest::Pause => Ok(self.with_player(Player::pause)),
            ControlRequest::Resume => Ok(self.with_player(Player::resume)),
            ControlRequest::Toggle => {
                let idle = {
                    let status = self.status.lock().expect("status lock");
                    !matches!(status.state, "playing" | "paused" | "loading")
                };
                if idle {
                    self.call(ControlRequest::ReadClipboard)
                } else {
                    Ok(self.with_player(Player::toggle))
                }
            }
            ControlRequest::Stop => Ok(self.with_player(Player::stop)),
            ControlRequest::Next => Ok(self.with_player(Player::next)),
            ControlRequest::Previous => Ok(self.with_player(Player::previous)),
            ControlRequest::Jump(index) => {
                let count = self.status.lock().expect("status lock").chunks.len();
                if index >= count {
                    return Err(ControlError::new(
                        400,
                        "invalid_request",
                        format!("index must be below the chunk count ({count})"),
                    ));
                }
                Ok(self.with_player(|player| player.jump(index)))
            }
            ControlRequest::Status => Ok(self.snapshot()),
            ControlRequest::ShowPanel => Err(ControlError::new(
                409,
                "no_panel",
                "The reading panel belongs to the tray app; `serve` has no window",
            )),
            ControlRequest::Quit => {
                self.with_player(Player::stop);
                self.stop.store(true, Ordering::Relaxed);
                Ok(self.snapshot())
            }
            ControlRequest::ApplySettings(settings) => {
                self.apply_settings(&settings);
                Ok(self.snapshot())
            }
        }
    }
}

// MARK: - Model downloads

/// The downloads a model choice needs, as `smart-voices download` fetches them.
#[must_use]
pub fn download_set(variant: crate::engine::ModelVariant) -> Vec<ModelId> {
    use crate::engine::ModelVariant;
    let mut models = vec![ModelId::English(variant)];
    if variant.uses_misaki() {
        models.push(ModelId::Misaki);
    }
    if variant == ModelVariant::Paradee {
        models.push(ModelId::EspeakData);
    }
    if variant == ModelVariant::Phonon {
        models.push(ModelId::PhononVoices);
    }
    models
}

/// One model download and how far it got.
#[derive(Debug, Clone)]
struct DownloadJob {
    label: &'static str,
    state: &'static str,
    done: u64,
    total: Option<u64>,
    error: Option<String>,
}

/// Background model downloads for the HTTP API.
#[derive(Default)]
pub struct Downloads {
    jobs: Arc<Mutex<Vec<DownloadJob>>>,
}

impl Downloads {
    /// Queue `models` on a worker thread, one after the other.
    pub fn start(&self, root: PathBuf, models: Vec<ModelId>) {
        let first = {
            let mut jobs = self.jobs.lock().expect("downloads lock");
            let first = jobs.len();
            jobs.extend(models.iter().map(|model| DownloadJob {
                label: model.label(),
                state: "queued",
                done: 0,
                total: None,
                error: None,
            }));
            first
        };
        let jobs = self.jobs.clone();
        std::thread::spawn(move || {
            for (offset, model) in models.into_iter().enumerate() {
                let index = first + offset;
                let update = |change: &mut dyn FnMut(&mut DownloadJob)| {
                    change(&mut jobs.lock().expect("downloads lock")[index]);
                };
                update(&mut |job| job.state = "downloading");
                let result = crate::models::download(&root, model, |done, total| {
                    update(&mut |job| {
                        job.done = done;
                        job.total = total;
                    });
                });
                match result {
                    Ok(_) => update(&mut |job| job.state = "done"),
                    Err(error) => {
                        let message = error.to_string();
                        eprintln!("error: model download failed: {message}");
                        update(&mut |job| {
                            job.state = "failed";
                            job.error = Some(message.clone());
                        });
                    }
                }
            }
        });
    }

    #[must_use]
    pub fn to_json(&self) -> Value {
        let jobs = self.jobs.lock().expect("downloads lock");
        let list: Vec<Value> = jobs
            .iter()
            .map(|job| {
                json!({
                    "model": job.label,
                    "state": job.state,
                    "doneBytes": job.done,
                    "totalBytes": job.total,
                    "error": job.error,
                })
            })
            .collect();
        let active = jobs
            .iter()
            .any(|job| matches!(job.state, "queued" | "downloading"));
        json!({"contract": 1, "active": active, "jobs": list})
    }
}

// MARK: - Discovery

/// Where a host publishes its address, next to the settings file.
fn discovery_path(settings_path: &Path, host: &str) -> Option<PathBuf> {
    Some(settings_path.parent()?.join(format!("{host}.control.json")))
}

/// Publish a host's address and token; only this user can read the file.
pub fn publish(
    settings_path: &Path,
    host: &str,
    url: &str,
    token: Option<&str>,
) -> Result<(), String> {
    let path = discovery_path(settings_path, host).ok_or("settings path has no parent")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("couldn't create {}: {e}", parent.display()))?;
    }
    let body = json!({"url": url, "token": token}).to_string();
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let write = |options: &std::fs::OpenOptions| -> std::io::Result<()> {
        use std::io::Write;
        let mut file = options.open(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(body.as_bytes())
    };
    write(&options).map_err(|e| format!("couldn't write {}: {e}", path.display()))
}

/// Remove a host's discovery file on a clean exit.
pub fn unpublish(settings_path: &Path, host: &str) {
    if let Some(path) = discovery_path(settings_path, host)
        && let Err(error) = std::fs::remove_file(&path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        eprintln!("error: couldn't remove {}: {error}", path.display());
    }
}

/// A random bearer token for the tray's own API.
pub fn random_token() -> Result<String, String> {
    use ring::rand::SecureRandom;
    let mut bytes = [0u8; 32];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| "couldn't generate an API token")?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

// MARK: - Client

/// A running host the CLI can talk to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub url: String,
    pub token: Option<String>,
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(timeout).build()
}

/// Longest wait for a host to answer a control request.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a host gets to prove it is alive during discovery.
const PROBE_TIMEOUT: Duration = Duration::from_secs(1);

fn is_alive(url: &str) -> bool {
    agent(PROBE_TIMEOUT)
        .get(&format!("{url}/v1/health"))
        .call()
        .is_ok()
}

/// The host to talk to: `SMART_VOICES_URL` (with `SMART_VOICES_TOKEN`), else a
/// live tray app, else a live `serve`. `None` means nothing is running.
#[must_use]
pub fn discover(settings_path: &Path) -> Option<Target> {
    if let Ok(url) = std::env::var("SMART_VOICES_URL")
        && !url.is_empty()
    {
        return Some(Target {
            url: url.trim_end_matches('/').to_string(),
            token: std::env::var("SMART_VOICES_TOKEN")
                .ok()
                .filter(|t| !t.is_empty()),
        });
    }
    ["tray", "serve"].into_iter().find_map(|host| {
        let text = std::fs::read_to_string(discovery_path(settings_path, host)?).ok()?;
        let value: Value = serde_json::from_str(&text).ok()?;
        let url = value["url"].as_str()?.to_string();
        is_alive(&url).then(|| Target {
            url,
            token: value["token"].as_str().map(str::to_string),
        })
    })
}

/// Call the API; the error is the server's own message.
pub fn call(
    target: &Target,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<Value, String> {
    let mut request = agent(CALL_TIMEOUT).request(method, &format!("{}{path}", target.url));
    if let Some(token) = &target.token {
        request = request.set("Authorization", &format!("Bearer {token}"));
    }
    request = request.set("Content-Type", "application/json");
    let result = match body {
        Some(body) => request.send_string(&body.to_string()),
        None if method == "GET" => request.call(),
        None => request.send_string("{}"),
    };
    match result {
        Ok(response) => response
            .into_string()
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .ok_or_else(|| format!("invalid reply from {}", target.url)),
        Err(ureq::Error::Status(status, response)) => {
            let message = response
                .into_string()
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
                .unwrap_or_else(|| format!("request failed with HTTP {status}"));
            Err(message)
        }
        Err(error) => Err(format!("couldn't reach {}: {error}", target.url)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::SinkEvent;
    use crate::player::SynthesizedAudio;
    use std::sync::atomic::AtomicBool;

    struct SilentSynth;
    impl Synth for SilentSynth {
        fn synthesize(&mut self, text: &str, _: &str, _: f32) -> Result<SynthesizedAudio, String> {
            Ok(SynthesizedAudio {
                samples: vec![0.0; text.len()],
                sample_rate: 24_000,
            })
        }
    }

    /// Accepts audio and never reports it finished, so a reading stays put.
    struct HeldOutput;
    impl Output for HeldOutput {
        fn enqueue(&mut self, _: u64, _: &[f32], _: u32) -> Result<(), String> {
            Ok(())
        }
        fn pause(&mut self) {}
        fn resume(&mut self) {}
        fn clear(&mut self) {}
        fn close_if_idle(&mut self) {}
        fn set_event_sender(&mut self, _: mpsc::Sender<SinkEvent>) {}
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "smart-voices-control-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn headless(dir: &Path, stop: Arc<AtomicBool>) -> HeadlessControl {
        let spawn = |settings: &Settings| {
            let config = crate::player::PlayerConfig {
                voice: settings.voice.clone(),
                speed: settings.speed,
                unload_after: None,
                sanitizer_options: crate::sanitize::SanitizerOptions::default(),
                pronunciations: crate::pronounce::Pronunciations::new(Vec::new()),
                normalize: settings.normalize_options(),
                persian_voice: settings.persian_voice,
                debug_latency: false,
            };
            let factory = || -> Result<Box<dyn Synth>, String> { Ok(Box::new(SilentSynth)) };
            let persian = || -> Result<Box<dyn Synth>, String> { Ok(Box::new(SilentSynth)) };
            Ok(Player::spawn(
                config,
                Box::new(factory),
                Box::new(persian),
                HeldOutput,
            ))
        };
        HeadlessControl::with_spawn(
            Arc::new(RwLock::new(Settings::default())),
            dir.join("settings.toml"),
            dir.to_path_buf(),
            stop,
            Box::new(spawn),
        )
    }

    fn wait_for(control: &HeadlessControl, state: &str) -> Value {
        for _ in 0..500 {
            let status = control.call(ControlRequest::Status).unwrap();
            if status["state"] == state {
                return status;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!(
            "never reached {state}: {}",
            control.call(ControlRequest::Status).unwrap()
        );
    }

    #[test]
    fn missing_voice_is_an_error_naming_the_download() {
        let dir = temp_dir("missing");
        let control = headless(&dir, Arc::default());
        let error = control
            .call(ControlRequest::Play("Hello there.".into()))
            .unwrap_err();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(error.status, 503);
        assert!(error.message.contains("/v1/downloads"), "{}", error.message);
    }

    #[test]
    fn status_folds_a_reading_from_start_to_finish() {
        let mut status = PlaybackStatus::default();
        status.apply(&PlayerEvent::Playlist(vec!["One.".into(), "Two.".into()]));
        status.apply(&PlayerEvent::State(PlayerState::Playing));
        status.apply(&PlayerEvent::Chunk {
            index: 1,
            count: 2,
            text: "Two.".into(),
            duration: Duration::from_secs(1),
        });
        let json = status.to_json("serve");
        assert_eq!(json["state"], "playing");
        assert_eq!(json["status"], "Reading 2 of 2");
        assert_eq!(json["text"], "Two.");
        status.apply(&PlayerEvent::State(PlayerState::Paused));
        assert_eq!(status.to_json("serve")["status"], "Paused");
        status.apply(&PlayerEvent::Error("No audio device".into()));
        let json = status.to_json("serve");
        assert_eq!(json["state"], "idle");
        assert_eq!(json["error"], "No audio device");
    }

    #[test]
    fn headless_host_has_no_panel_and_quit_stops_the_server() {
        let dir = temp_dir("quit");
        let stop = Arc::new(AtomicBool::new(false));
        let control = headless(&dir, stop.clone());
        assert_eq!(
            control.call(ControlRequest::ShowPanel).unwrap_err().status,
            409
        );
        control.call(ControlRequest::Quit).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(stop.load(Ordering::Relaxed));
    }

    #[test]
    fn headless_transport_controls_a_reading() {
        let dir = temp_dir("transport");
        for model in download_set(crate::engine::ModelVariant::Fast) {
            let model_dir = crate::models::model_dir(&dir, model);
            std::fs::create_dir_all(&model_dir).unwrap();
            for file in model.required_files() {
                let path = model_dir.join(file);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, b"").unwrap();
            }
        }
        let control = headless(&dir, Arc::default());
        control
            .call(ControlRequest::Play(
                "First one here. Second one here.".into(),
            ))
            .unwrap();
        let playing = wait_for(&control, "playing");
        assert_eq!(playing["host"], "serve");
        assert!(playing["count"].as_u64().unwrap() >= 1);
        control.call(ControlRequest::Pause).unwrap();
        wait_for(&control, "paused");
        control.call(ControlRequest::Resume).unwrap();
        wait_for(&control, "playing");
        control.call(ControlRequest::Stop).unwrap();
        wait_for(&control, "idle");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn publish_is_private_and_unpublish_removes_it() {
        let dir = temp_dir("publish");
        let settings = dir.join("settings.toml");
        publish(&settings, "tray", "http://127.0.0.1:1", Some("secret")).unwrap();
        let path = discovery_path(&settings, "tray").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // Nothing is listening on port 1, so discovery treats it as stale.
        assert_eq!(discover(&settings), None);
        unpublish(&settings, "tray");
        assert!(!path.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
