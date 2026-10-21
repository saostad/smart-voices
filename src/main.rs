#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use smart_voices::app;
use smart_voices::audio::AudioSink;
use smart_voices::clipboard::{self, ClipboardEvent, MIN_TEXT_LEN};
use smart_voices::engine::{ModelVariant, PersianVoice};
use smart_voices::models::{self, ModelError, ModelId};
use smart_voices::pipeline::prepare;
use smart_voices::player::{Player, PlayerConfig, PlayerEvent, Synth};
use smart_voices::pronounce::{Pronunciations, pronunciations_path};
use smart_voices::reader::bmp::encode_bmp_24;
use smart_voices::reader::render::{Frame, Painter, Theme};
use smart_voices::sanitize::SanitizerOptions;
use smart_voices::settings::{Settings, settings_path};

const USAGE: &str = "Usage: smart-voices (starts the tray app)\n       smart-voices speak \"<text>\" [--voice <name>] [--speed <f>] [--model fast|small|phonon] [--persian-voice <id>] [--verbose]\n       smart-voices download [--model fast|small|phonon] [--persian] [--persian-voice <id>]\n       smart-voices clean [<file>]\n       smart-voices watch [--auto-read]\n       smart-voices reader-preview <out.bmp> [--dark] [--width 440] [--height 300] [--scale 2]";

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
        Some("reader-preview") => reader_preview(&args[1..]),
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
    persian_voice: Option<PersianVoice>,
    verbose: bool,
}

fn parse_speak(args: &[String]) -> Option<SpeakOptions> {
    let mut text: Option<String> = None;
    let mut voice: Option<String> = None;
    let mut speed: Option<f32> = None;
    let mut model: Option<ModelVariant> = None;
    let mut persian_voice: Option<PersianVoice> = None;
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
            "--persian-voice" => {
                i += 1;
                persian_voice = Some(PersianVoice::parse(args.get(i)?)?);
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
        persian_voice,
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
    let voice_flag = options.voice.is_some();
    let voice = options.voice.unwrap_or(settings.voice);
    let speed = options.speed.unwrap_or(settings.speed);
    let model = options.model.unwrap_or(settings.model);
    let persian_voice = options.persian_voice.unwrap_or(settings.persian_voice);
    if voice_flag && model == ModelVariant::Phonon {
        eprintln!("Note: --voice is ignored with --model phonon; Phonon uses its own voice.");
    }

    // Download whichever models this text needs; English-only text never
    // downloads the Persian voice.
    let chunks = prepare(&options.text, &SanitizerOptions::default(), &pronunciations);
    for needed in models::models_for_chunks(
        &chunks,
        model,
        persian_voice,
        smart_voices::phonon::DEFAULT_PHONON_VOICE,
    ) {
        if let Err(err) = download_with_progress(&models_root, needed) {
            eprintln!("Error: {err}");
            return 1;
        }
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
        persian_voice,
        debug_latency: false,
    };
    let persian_root = models_root.clone();
    let factory = move || -> Result<Box<dyn Synth>, String> {
        smart_voices::player::load_english_engine(&models_root, model)
    };
    let persian_factory = move || -> Result<Box<dyn Synth>, String> {
        smart_voices::player::load_persian_engine(&persian_root, persian_voice)
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
            PlayerEvent::State(_) | PlayerEvent::Playlist(_) | PlayerEvent::Latency { .. } => {}
        }
    }
    eprintln!("Error: The player stopped before finishing.");
    1
}

struct DownloadOptions {
    kokoro: Option<ModelVariant>,
    persian: Option<PersianVoice>,
}

fn parse_download(args: &[String]) -> Option<DownloadOptions> {
    let mut kokoro: Option<ModelVariant> = None;
    let mut persian: Option<PersianVoice> = None;
    let mut want_persian = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--model" => {
                i += 1;
                kokoro = Some(ModelVariant::parse(args.get(i)?)?);
            }
            "--persian" => {
                want_persian = true;
            }
            "--persian-voice" => {
                i += 1;
                persian = Some(PersianVoice::parse(args.get(i)?)?);
            }
            _ => return None,
        }
        i += 1;
    }
    // `--persian-voice <id>` alone implies the Persian download; `--persian`
    // alone means the default voice. A bare `download` keeps fetching Fast.
    if want_persian && persian.is_none() {
        persian = Some(PersianVoice::default());
    }
    if kokoro.is_none() && persian.is_none() {
        kokoro = Some(ModelVariant::Fast);
    }
    Some(DownloadOptions { kokoro, persian })
}

fn download_cmd(args: &[String]) -> i32 {
    let Some(options) = parse_download(args) else {
        eprintln!("{USAGE}");
        return 2;
    };
    let Some(models_root) = models_root() else {
        eprintln!(
            "Error: Couldn't find the models folder: SMART_VOICES_MODELS is not set and no data directory is available."
        );
        return 1;
    };
    let mut models = Vec::with_capacity(2);
    if let Some(variant) = options.kokoro {
        models.push(ModelId::English(variant));
    }
    if let Some(voice) = options.persian {
        models.push(ModelId::Persian(voice));
    }
    for model in models {
        match download_with_progress(&models_root, model) {
            Ok(dir) => eprintln!("Voice model ready at {}", dir.display()),
            Err(err) => {
                eprintln!("Error: {err}");
                return 1;
            }
        }
    }
    0
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

/// Sample reading for the `reader-preview` dev command.
const PREVIEW_CHUNKS: [&str; 6] = [
    "Smart Voices reads copied text aloud.",
    "This panel shows the paragraph being read and highlights the current word as the voice speaks it.",
    "Click any paragraph to jump there, or use the buttons below to skip back and forth.",
    "سلام! این یک متن فارسی برای آزمایش است.",
    "Tables are read row by row and code blocks are announced.",
    "Everything runs on your computer.",
];

struct PreviewOptions {
    path: String,
    dark: bool,
    width: u32,
    height: u32,
    scale: f32,
}

fn parse_preview(args: &[String]) -> Option<PreviewOptions> {
    let mut path: Option<String> = None;
    let mut dark = false;
    let mut width = 440;
    let mut height = 300;
    let mut scale = 2.0;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--dark" => dark = true,
            "--width" => {
                i += 1;
                width = args.get(i)?.parse::<u32>().ok()?;
                if width == 0 {
                    return None;
                }
            }
            "--height" => {
                i += 1;
                height = args.get(i)?.parse::<u32>().ok()?;
                if height == 0 {
                    return None;
                }
            }
            "--scale" => {
                i += 1;
                scale = args.get(i)?.parse::<f32>().ok()?;
                if !scale.is_finite() || scale <= 0.0 {
                    return None;
                }
            }
            flag if flag.starts_with("--") => return None,
            positional => {
                if path.is_some() {
                    return None;
                }
                path = Some(positional.to_string());
            }
        }
        i += 1;
    }
    Some(PreviewOptions {
        path: path?,
        dark,
        width,
        height,
        scale,
    })
}

/// Render the sample reading offscreen and write it as a 24-bit BMP.
fn reader_preview(args: &[String]) -> i32 {
    let options = match parse_preview(args) {
        Some(options) => options,
        None => {
            eprintln!("{USAGE}");
            return 2;
        }
    };
    // Width/height are logical; the BMP holds physical pixels.
    let width_px = ((options.width as f32 * options.scale).round() as u32).max(1);
    let height_px = ((options.height as f32 * options.scale).round() as u32).max(1);
    let theme = if options.dark {
        Theme::dark()
    } else {
        Theme::light()
    };
    let mut painter = match Painter::new() {
        Ok(painter) => painter,
        Err(err) => {
            eprintln!("Error: Couldn't open the reading panel: {err}");
            return 1;
        }
    };
    painter.resize(width_px, height_px, options.scale);
    let chunks: Vec<String> = PREVIEW_CHUNKS.iter().map(ToString::to_string).collect();
    painter.set_playlist(&chunks);
    painter.set_highlight(Some(1), Some(3));
    painter.scroll_to_current();
    // Nudge down so the Persian sample row is fully visible at the default
    // size (clamped, so harmless at other sizes).
    painter.scroll_by(20.0 * options.scale);
    let frame = Frame {
        label: "Sentence 2 of 6",
        progress: 2.0 / 6.0,
        can_previous: true,
        can_next: true,
        playing: true,
        can_play_pause: true,
        theme: &theme,
    };
    let count = width_px as u64 * height_px as u64;
    let mut pixels: Vec<u32> = Vec::new();
    if count > usize::MAX as u64 || pixels.try_reserve(count as usize).is_err() {
        eprintln!(
            "Error: Couldn't write {}: the image is too large ({width_px}×{height_px}).",
            options.path
        );
        return 1;
    }
    pixels.resize(count as usize, 0);
    painter.paint(&frame, &mut pixels);
    let bmp = encode_bmp_24(width_px, height_px, &pixels);
    match std::fs::write(&options.path, &bmp) {
        Ok(()) => {
            println!("Wrote {} ({width_px}×{height_px})", options.path);
            0
        }
        Err(err) => {
            eprintln!("Error: Couldn't write {}: {err}", options.path);
            1
        }
    }
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
    persian_voice: PersianVoice,
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
            persian_voice: settings.persian_voice,
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
        // Download whichever models this text needs (no-op when installed),
        // like `speak` and the tray app.
        if let Err(message) = self.ensure_models(&text) {
            eprintln!("Error: {message}");
            return;
        }
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

    fn ensure_models(&self, text: &str) -> Result<(), String> {
        let models_root = models_root().ok_or_else(|| {
            "Couldn't find the models folder: SMART_VOICES_MODELS is not set and no data directory is available."
                .to_string()
        })?;
        let chunks = prepare(text, &SanitizerOptions::default(), &self.pronunciations);
        for needed in models::models_for_chunks(
            &chunks,
            self.model,
            self.persian_voice,
            smart_voices::phonon::DEFAULT_PHONON_VOICE,
        ) {
            download_with_progress(&models_root, needed).map_err(|err| err.to_string())?;
        }
        Ok(())
    }

    fn spawn(&self) -> Result<Player, String> {
        let models_root = models_root().ok_or_else(|| {
            "Couldn't find the models folder: SMART_VOICES_MODELS is not set and no data directory is available."
                .to_string()
        })?;
        let sink = AudioSink::new().map_err(|err| err.to_string())?;
        let persian_voice = self.persian_voice;
        let config = PlayerConfig {
            voice: self.voice.clone(),
            speed: self.speed,
            unload_after: self.unload_after,
            sanitizer_options: SanitizerOptions::default(),
            pronunciations: self.pronunciations.clone(),
            persian_voice,
            debug_latency: false,
        };
        let model = self.model;
        let persian_root = models_root.clone();
        let factory = move || -> Result<Box<dyn Synth>, String> {
            smart_voices::player::load_english_engine(&models_root, model)
        };
        let persian_factory = move || -> Result<Box<dyn Synth>, String> {
            smart_voices::player::load_persian_engine(&persian_root, persian_voice)
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
