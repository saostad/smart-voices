#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use smart_voices::app;
use smart_voices::audio::AudioSink;
use smart_voices::clipboard::{self, ClipboardEvent, MIN_TEXT_LEN};
use smart_voices::engine::{KokoroEngine, ModelVariant, PersianVoice, PiperEngine};
use smart_voices::models::{self, ModelError, ModelId};
use smart_voices::pipeline::prepare;
use smart_voices::player::{Player, PlayerConfig, PlayerEvent, Synth};
use smart_voices::pronounce::{Pronunciations, pronunciations_path};
use smart_voices::sanitize::SanitizerOptions;
use smart_voices::settings::{Settings, settings_path};

const USAGE: &str = "Usage: smart-voices (starts the tray app)\n       smart-voices speak \"<text>\" [--voice <name>] [--speed <f>] [--model fast|small] [--verbose]\n       smart-voices download [--model fast|small]\n       smart-voices clean [<file>]\n       smart-voices watch [--auto-read]";

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Release Windows builds run without a console; CLI commands attach to
    // the parent console so their output is still visible.
    #[cfg(windows)]
    if !args.is_empty() {
        attach_parent_console();
    }
    match args.first().map(String::as_str) {
        None => match acquire_instance_lock() {
            InstanceLock::Held(held) => {
                // Held until the app exits.
                let _held = held;
                app::run()
            }
            InstanceLock::AlreadyRunning => {
                println!("Smart Voices is already running.");
                0
            }
            // Best effort: a missing lock must not stop the app.
            InstanceLock::Unavailable => app::run(),
        },
        Some("speak") => speak(&args[1..]),
        Some("download") => download_cmd(&args[1..]),
        Some("clean") => clean(&args[1..]),
        Some("watch") => watch_cmd(&args[1..]),
        _ => {
            eprintln!("{USAGE}");
            2
        }
    }
}

#[cfg(windows)]
fn attach_parent_console() {
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    // SAFETY: `AttachConsole` with the parent pseudo-handle only affects this
    // process's console attachment; failure (no parent console) is ignored.
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

/// Single-instance lock state for a bare (tray-app) launch.
enum InstanceLock {
    /// This launch holds the lock; the file must stay alive.
    Held(std::fs::File),
    /// Another instance already holds the lock.
    AlreadyRunning,
    /// The lock couldn't be used (no data dir, permissions); run anyway.
    Unavailable,
}

/// Take the single-instance lock in the data dir (exclusive, non-blocking).
fn acquire_instance_lock() -> InstanceLock {
    let Some(dir) = dirs::data_dir().map(|dir| dir.join("smart-voices")) else {
        return InstanceLock::Unavailable;
    };
    if std::fs::create_dir_all(&dir).is_err() {
        return InstanceLock::Unavailable;
    }
    let file = match std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("app.lock"))
    {
        Ok(file) => file,
        Err(_) => return InstanceLock::Unavailable,
    };
    match file.try_lock() {
        Ok(()) => InstanceLock::Held(file),
        Err(std::fs::TryLockError::WouldBlock) => InstanceLock::AlreadyRunning,
        Err(_) => InstanceLock::Unavailable,
    }
}

struct SpeakOptions {
    text: String,
    voice: Option<String>,
    speed: Option<f32>,
    model: Option<ModelVariant>,
    verbose: bool,
}

fn parse_speak(args: &[String]) -> Option<SpeakOptions> {
    let mut text: Option<String> = None;
    let mut voice: Option<String> = None;
    let mut speed: Option<f32> = None;
    let mut model: Option<ModelVariant> = None;
    let mut verbose = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--voice" => {
                i += 1;
                voice = Some(args.get(i)?.to_string());
            }
            "--speed" => {
                i += 1;
                speed = Some(args.get(i)?.parse::<f32>().ok()?);
            }
            "--model" => {
                i += 1;
                model = Some(ModelVariant::parse(args.get(i)?)?);
            }
            "--verbose" => {
                verbose = true;
            }
            flag if flag.starts_with("--") => return None,
            positional => {
                if text.is_some() {
                    return None;
                }
                text = Some(positional.to_string());
            }
        }
        i += 1;
    }
    Some(SpeakOptions {
        text: text?,
        voice,
        speed,
        model,
        verbose,
    })
}

fn models_root() -> Option<PathBuf> {
    match std::env::var("SMART_VOICES_MODELS") {
        Ok(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
        _ => dirs::data_dir().map(|dir| dir.join("smart-voices").join("models")),
    }
}

fn speak(args: &[String]) -> i32 {
    let options = match parse_speak(args) {
        Some(options) => options,
        None => {
            eprintln!("{USAGE}");
            return 2;
        }
    };
    let Some(models_root) = models_root() else {
        eprintln!(
            "Error: Couldn't find the models folder: SMART_VOICES_MODELS is not set and no data directory is available."
        );
        return 1;
    };
    let (settings, settings_err) = settings_path()
        .map(|path| Settings::load(&path))
        .unwrap_or((Settings::default(), None));
    if let Some(err) = &settings_err {
        eprintln!("Warning: {err}");
    }
    let (pronunciations, pronunciations_err) = pronunciations_path()
        .map(|path| Pronunciations::load(&path))
        .unwrap_or((Pronunciations::with_defaults(), None));
    if let Some(err) = &pronunciations_err {
        eprintln!("Warning: {err}");
    }
    let voice = options.voice.unwrap_or(settings.voice);
    let speed = options.speed.unwrap_or(settings.speed);
    let model = options.model.unwrap_or(settings.model);

    if !models::is_installed(&models_root, ModelId::Kokoro(model))
        && let Err(err) = download_with_progress(&models_root, ModelId::Kokoro(model))
    {
        eprintln!("Error: {err}");
        return 1;
    }

    let sink = match AudioSink::new() {
        Ok(sink) => sink,
        Err(err) => {
            eprintln!("Error: {err}");
            return 1;
        }
    };
    let unload_after = if settings.unload_after_minutes == 0 {
        None
    } else {
        Some(Duration::from_secs(
            u64::from(settings.unload_after_minutes) * 60,
        ))
    };
    let config = PlayerConfig {
        voice,
        speed,
        unload_after,
        sanitizer_options: SanitizerOptions::default(),
        pronunciations,
        // Wired to settings in the next item; Amir until then.
        persian_voice: PersianVoice::Amir,
    };
    let persian_root = models_root.clone();
    let factory = move || -> Result<Box<dyn Synth>, String> {
        match KokoroEngine::load(&models_root, model) {
            Ok(engine) => Ok(Box::new(engine) as Box<dyn Synth>),
            Err(err) => Err(err.to_string()),
        }
    };
    let persian_factory = move || -> Result<Box<dyn Synth>, String> {
        match PiperEngine::load(&persian_root, PersianVoice::Amir) {
            Ok(engine) => Ok(Box::new(engine) as Box<dyn Synth>),
            Err(err) => Err(err.to_string()),
        }
    };
    let (player, events) =
        Player::spawn(config, Box::new(factory), Box::new(persian_factory), sink);
    let started = Instant::now();
    player.play(options.text);
    let mut first_audio = true;
    for event in events {
        match event {
            PlayerEvent::Chunk { text, .. } => {
                if first_audio {
                    eprintln!("First audio after {} ms", started.elapsed().as_millis());
                    first_audio = false;
                }
                if options.verbose {
                    eprintln!("{text}");
                }
            }
            PlayerEvent::Finished => return 0,
            PlayerEvent::Error(message) => {
                eprintln!("Error: {message}");
                return 1;
            }
            PlayerEvent::State(_) => {}
        }
    }
    eprintln!("Error: The player stopped before finishing.");
    1
}

fn parse_download(args: &[String]) -> Option<ModelVariant> {
    let mut model = ModelVariant::Fast;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--model" => {
                i += 1;
                model = ModelVariant::parse(args.get(i)?)?;
            }
            _ => return None,
        }
        i += 1;
    }
    Some(model)
}

fn download_cmd(args: &[String]) -> i32 {
    let Some(model) = parse_download(args) else {
        eprintln!("{USAGE}");
        return 2;
    };
    let Some(models_root) = models_root() else {
        eprintln!(
            "Error: Couldn't find the models folder: SMART_VOICES_MODELS is not set and no data directory is available."
        );
        return 1;
    };
    match download_with_progress(&models_root, ModelId::Kokoro(model)) {
        Ok(dir) => {
            eprintln!("Voice model ready at {}", dir.display());
            0
        }
        Err(err) => {
            eprintln!("Error: {err}");
            1
        }
    }
}

/// Read Markdown from `file` (or stdin) and print one line per
/// speech chunk as `<pause>\t<text>` (display text, no file I/O).
fn clean(args: &[String]) -> i32 {
    let text = match args.len() {
        0 => match std::io::read_to_string(std::io::stdin()) {
            Ok(text) => text,
            Err(err) => {
                eprintln!("Error: Couldn't read from standard input: {err}.");
                return 1;
            }
        },
        1 => match std::fs::read_to_string(&args[0]) {
            Ok(text) => text,
            Err(err) => {
                eprintln!("Error: Couldn't read {}: {err}.", args[0]);
                return 1;
            }
        },
        _ => {
            eprintln!("{USAGE}");
            return 2;
        }
    };
    let pronunciations = Pronunciations::with_defaults();
    for chunk in prepare(&text, &SanitizerOptions::default(), &pronunciations) {
        println!("{}\t{}", chunk.pause_after.label(), chunk.text);
    }
    0
}

/// Watch the clipboard and announce new text; with auto-read, read it aloud.
/// The player (and the voice model) starts lazily on the first copy, so plain
/// `watch` works without audio or a downloaded model. Ctrl+C ends the process
/// (default SIGINT termination); nothing is half-written: settings and
/// pronunciations are only read here (missing pronunciations are seeded once
/// at startup via an atomic tmp-file rename).
fn watch_cmd(args: &[String]) -> i32 {
    let mut auto_read_flag = false;
    for arg in args {
        if arg == "--auto-read" {
            auto_read_flag = true;
        } else {
            eprintln!("{USAGE}");
            return 2;
        }
    }
    let (settings, settings_err) = settings_path()
        .map(|path| Settings::load(&path))
        .unwrap_or((Settings::default(), None));
    if let Some(err) = &settings_err {
        eprintln!("Warning: {err}");
    }
    let (pronunciations, pronunciations_err) = pronunciations_path()
        .map(|path| Pronunciations::load(&path))
        .unwrap_or((Pronunciations::with_defaults(), None));
    if let Some(err) = &pronunciations_err {
        eprintln!("Warning: {err}");
    }
    let auto_read = auto_read_flag || settings.auto_read;
    let (events_tx, events) = std::sync::mpsc::channel();
    let _watcher = match clipboard::watch(events_tx) {
        Ok(watcher) => watcher,
        Err(err) => {
            eprintln!("Error: {err}");
            return 1;
        }
    };
    let mut player = LazyPlayer::new(&settings, pronunciations);
    println!("Watching the clipboard. Press Ctrl+C to quit.");
    for event in events {
        match event {
            ClipboardEvent::Changed { source_app } => {
                let app = source_app.as_deref().unwrap_or("an app");
                println!("Copied text from {app}");
                if !auto_read {
                    continue;
                }
                match clipboard::read_text() {
                    Ok(Some(text)) => {
                        if text.chars().count() < MIN_TEXT_LEN {
                            continue;
                        }
                        player.play(text);
                    }
                    Ok(None) => {}
                    Err(err) => eprintln!("Error: {err}"),
                }
            }
            ClipboardEvent::Error(message) => {
                eprintln!("Error: {message}");
                return 1;
            }
        }
    }
    // The channel only closes when the watcher thread is gone.
    eprintln!("Error: The clipboard watcher stopped unexpectedly.");
    1
}

/// Player spawned on first use (auto-read only), then reused; `play`
/// interrupts the current reading, like Swift's auto-read.
struct LazyPlayer {
    player: Option<Player>,
    voice: String,
    speed: f32,
    model: ModelVariant,
    unload_after: Option<Duration>,
    pronunciations: Pronunciations,
}

impl LazyPlayer {
    fn new(settings: &Settings, pronunciations: Pronunciations) -> Self {
        Self {
            player: None,
            voice: settings.voice.clone(),
            speed: settings.speed,
            model: settings.model,
            unload_after: if settings.unload_after_minutes == 0 {
                None
            } else {
                Some(Duration::from_secs(
                    u64::from(settings.unload_after_minutes) * 60,
                ))
            },
            pronunciations,
        }
    }

    fn play(&mut self, text: String) {
        if self.player.is_none() {
            match self.spawn() {
                Ok(player) => self.player = Some(player),
                Err(message) => {
                    eprintln!("Error: {message}");
                    return;
                }
            }
        }
        if let Some(player) = &self.player {
            player.play(text);
        }
    }

    fn spawn(&self) -> Result<Player, String> {
        let models_root = models_root().ok_or_else(|| {
            "Couldn't find the models folder: SMART_VOICES_MODELS is not set and no data directory is available."
                .to_string()
        })?;
        if !models::is_installed(&models_root, ModelId::Kokoro(self.model)) {
            download_with_progress(&models_root, ModelId::Kokoro(self.model))
                .map_err(|err| err.to_string())?;
        }
        let sink = AudioSink::new().map_err(|err| err.to_string())?;
        let config = PlayerConfig {
            voice: self.voice.clone(),
            speed: self.speed,
            unload_after: self.unload_after,
            sanitizer_options: SanitizerOptions::default(),
            pronunciations: self.pronunciations.clone(),
            // Wired to settings in the next item; Amir until then.
            persian_voice: PersianVoice::Amir,
        };
        let model = self.model;
        let persian_root = models_root.clone();
        let factory = move || -> Result<Box<dyn Synth>, String> {
            match KokoroEngine::load(&models_root, model) {
                Ok(engine) => Ok(Box::new(engine) as Box<dyn Synth>),
                Err(err) => Err(err.to_string()),
            }
        };
        let persian_factory = move || -> Result<Box<dyn Synth>, String> {
            match PiperEngine::load(&persian_root, PersianVoice::Amir) {
                Ok(engine) => Ok(Box::new(engine) as Box<dyn Synth>),
                Err(err) => Err(err.to_string()),
            }
        };
        let (player, player_events) =
            Player::spawn(config, Box::new(factory), Box::new(persian_factory), sink);
        // Playback problems are reported but don't stop the watcher.
        std::thread::spawn(move || {
            for event in player_events {
                if let PlayerEvent::Error(message) = event {
                    eprintln!("Error: {message}");
                }
            }
        });
        Ok(player)
    }
}

/// Download `model` unless already installed, with a one-line stderr
/// progress display. Returns the install dir.
fn download_with_progress(root: &Path, model: ModelId) -> Result<PathBuf, ModelError> {
    if models::is_installed(root, model) {
        return Ok(models::model_dir(root, model));
    }
    let label = model.label();
    let result = models::download(root, model, |done, total| {
        const MB: f64 = 1_000_000.0;
        match total {
            Some(total) => eprint!(
                "\rDownloading {label}: {:.1}/{:.1} MB",
                done as f64 / MB,
                total as f64 / MB
            ),
            None => eprint!("\rDownloading {label}: {:.1} MB", done as f64 / MB),
        }
    });
    eprintln!();
    result
}
