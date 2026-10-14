//! Tray application: a menu-bar icon driven by the player and the
//! clipboard watcher. The winit event loop runs with `ControlFlow::Wait` (zero
//! CPU when idle) and creates no windows; player, clipboard, tray and menu
//! events are forwarded into it from small forwarder threads and the
//! tray/menu global handlers. All UI updates run on the main thread.

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
#[cfg(target_os = "macos")]
use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
use winit::window::WindowId;

use crate::audio::AudioSink;
use crate::clipboard::{self, ClipboardEvent, MIN_TEXT_LEN};
use crate::engine::{KokoroEngine, ModelVariant};
use crate::icon;
use crate::models;
use crate::player::{Player, PlayerConfig, PlayerEvent, PlayerState, Synth};
use crate::pronounce::{Pronunciations, pronunciations_path};
use crate::sanitize::SanitizerOptions;
use crate::settings::{Settings, settings_path};

/// Events forwarded into the event loop from worker threads and the
/// tray/menu global handlers.
#[derive(Debug)]
enum AppEvent {
    Player(PlayerEvent),
    Clipboard(ClipboardEvent),
    Tray(TrayIconEvent),
    Menu(MenuEvent),
    DownloadProgress(u64, Option<u64>),
    DownloadDone(Result<(), String>),
}

/// Longest status-line text, in characters.
const MAX_STATUS_CHARS: usize = 80;

/// Truncate a status message to [`MAX_STATUS_CHARS`] characters
/// (char-boundary safe).
fn truncate_status(text: &str) -> String {
    if text.chars().count() <= MAX_STATUS_CHARS {
        return text.to_string();
    }
    text.chars().take(MAX_STATUS_CHARS).collect()
}

/// Run the tray app. Returns the process exit code.
pub fn run() -> i32 {
    let settings_file = settings_path();
    let (settings, settings_err) = settings_file
        .as_ref()
        .map(|path| Settings::load(path))
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
    let models_root = match models_root() {
        Some(root) => root,
        None => {
            eprintln!(
                "Error: Couldn't find the models folder: SMART_VOICES_MODELS is not set and no data directory is available."
            );
            return 1;
        }
    };

    let mut builder = EventLoop::<AppEvent>::with_user_event();
    #[cfg(target_os = "macos")]
    {
        builder.with_activation_policy(ActivationPolicy::Accessory);
        // No app menu bar for a tray-only app.
        builder.with_default_menu(false);
    }
    let event_loop = match builder.build() {
        Ok(event_loop) => event_loop,
        Err(err) => {
            eprintln!("Error: Couldn't start the app event loop: {err}.");
            return 1;
        }
    };
    let proxy = event_loop.create_proxy();
    let tray_proxy = proxy.clone();
    TrayIconEvent::set_event_handler(Some(move |event| {
        let _ = tray_proxy.send_event(AppEvent::Tray(event));
    }));
    let menu_proxy = proxy.clone();
    MenuEvent::set_event_handler(Some(move |event| {
        let _ = menu_proxy.send_event(AppEvent::Menu(event));
    }));

    let mut app = App::new(proxy, settings, settings_file, pronunciations, models_root);
    // The clipboard watcher runs on its own thread; its events are forwarded
    // into the loop. A failure here (e.g. Wayland without X11) still shows
    // the tray so manual Read Clipboard keeps working.
    if app.settings.watch_clipboard
        && let Err(message) = app.start_watcher()
    {
        eprintln!("Error: {message}");
        app.startup_error = Some(message);
    }
    match event_loop.run_app(&mut app) {
        Ok(()) => app.exit_code,
        Err(err) => {
            eprintln!("Error: The app event loop failed: {err}.");
            1
        }
    }
}

fn models_root() -> Option<PathBuf> {
    match std::env::var("SMART_VOICES_MODELS") {
        Ok(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
        _ => dirs::data_dir().map(|dir| dir.join("smart-voices").join("models")),
    }
}

/// Tray app state. Everything here is touched only on the main thread.
struct App {
    proxy: EventLoopProxy<AppEvent>,
    settings: Settings,
    settings_path: Option<PathBuf>,
    pronunciations: Pronunciations,
    models_root: PathBuf,
    model: ModelVariant,
    player: Option<Player>,
    watcher: Option<clipboard::Watcher>,
    /// Watcher error to show in the status line once the menu exists.
    startup_error: Option<String>,
    tray: Option<TrayIcon>,
    status_item: Option<MenuItem>,
    read_item: Option<MenuItem>,
    pause_item: Option<MenuItem>,
    stop_item: Option<MenuItem>,
    prev_item: Option<MenuItem>,
    next_item: Option<MenuItem>,
    watch_item: Option<CheckMenuItem>,
    autoread_item: Option<CheckMenuItem>,
    quit_item: Option<MenuItem>,
    /// Reading or paused (transport items enabled).
    active: bool,
    /// Last chunk position, to restore "Reading i of n" on resume.
    last_chunk: Option<(usize, usize)>,
    paused: bool,
    /// Whether the status line currently shows an error (kept until the next
    /// action; the `Idle` state right after an `Error` must not clear it).
    showing_error: bool,
    badged: bool,
    /// A model download is running on a worker thread.
    downloading: bool,
    /// Text to play once the running download finishes.
    pending_text: Option<String>,
    exit_code: i32,
}

impl App {
    fn new(
        proxy: EventLoopProxy<AppEvent>,
        settings: Settings,
        settings_path: Option<PathBuf>,
        pronunciations: Pronunciations,
        models_root: PathBuf,
    ) -> Self {
        let model = settings.model;
        Self {
            proxy,
            settings,
            settings_path,
            pronunciations,
            models_root,
            model,
            player: None,
            watcher: None,
            startup_error: None,
            tray: None,
            status_item: None,
            read_item: None,
            pause_item: None,
            stop_item: None,
            prev_item: None,
            next_item: None,
            watch_item: None,
            autoread_item: None,
            quit_item: None,
            active: false,
            last_chunk: None,
            paused: false,
            showing_error: false,
            badged: false,
            downloading: false,
            pending_text: None,
            exit_code: 0,
        }
    }

    /// Start the clipboard watcher unless already running, forwarding its
    /// events into the loop from a small thread.
    fn start_watcher(&mut self) -> Result<(), String> {
        if self.watcher.is_some() {
            return Ok(());
        }
        let (events_tx, events_rx) = mpsc::channel();
        let watcher = clipboard::watch(events_tx).map_err(|err| err.to_string())?;
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            for event in events_rx {
                if proxy.send_event(AppEvent::Clipboard(event)).is_err() {
                    break;
                }
            }
        });
        self.watcher = Some(watcher);
        Ok(())
    }

    /// Persist the settings after a menu-check change; failures show in the
    /// status line.
    fn save_settings(&mut self) {
        let result = match &self.settings_path {
            Some(path) => self.settings.save(path),
            None => return,
        };
        if let Err(err) = result {
            self.show_error(&err.to_string());
        }
    }

    /// Create the menu and tray icon on the main thread (tray-icon requires
    /// this on macOS). Called once from `resumed`.
    fn create_tray(&mut self) -> Result<(), String> {
        let menu = Menu::new();
        let status_item = MenuItem::new("Ready", false, None);
        let read_item = MenuItem::new("Read Clipboard", true, None);
        let pause_item = MenuItem::new("Pause", false, None);
        let stop_item = MenuItem::new("Stop", false, None);
        let prev_item = MenuItem::new("Previous Sentence", false, None);
        let next_item = MenuItem::new("Next Sentence", false, None);
        let watch_item = CheckMenuItem::new(
            "Watch the Clipboard",
            true,
            self.settings.watch_clipboard,
            None,
        );
        let autoread_item = CheckMenuItem::new(
            "Read New Copies Automatically",
            true,
            self.settings.auto_read,
            None,
        );
        let quit_item = MenuItem::new("Quit Smart Voices", true, None);
        menu.append(&status_item)
            .and_then(|()| menu.append(&PredefinedMenuItem::separator()))
            .and_then(|()| menu.append(&read_item))
            .and_then(|()| menu.append(&pause_item))
            .and_then(|()| menu.append(&stop_item))
            .and_then(|()| menu.append(&prev_item))
            .and_then(|()| menu.append(&next_item))
            .and_then(|()| menu.append(&PredefinedMenuItem::separator()))
            .and_then(|()| menu.append(&watch_item))
            .and_then(|()| menu.append(&autoread_item))
            .and_then(|()| menu.append(&PredefinedMenuItem::separator()))
            .and_then(|()| menu.append(&quit_item))
            .map_err(|err| format!("Couldn't build the tray menu: {err}."))?;
        let icon = tray_icon(&icon::speaker())
            .ok_or_else(|| "Couldn't build the tray icon: invalid image data.".to_string())?;
        let builder = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Smart Voices")
            .with_menu_on_left_click(false);
        // macOS draws the icon as a template so it adapts to light/dark menu
        // bars; elsewhere the white glyph suits the usual dark trays.
        #[cfg(target_os = "macos")]
        let builder = builder.with_icon_templated(icon);
        #[cfg(not(target_os = "macos"))]
        let builder = builder.with_icon(icon);
        let tray = builder
            .build()
            .map_err(|err| format!("Couldn't create the tray icon: {err}."))?;
        self.status_item = Some(status_item);
        self.read_item = Some(read_item);
        self.pause_item = Some(pause_item);
        self.stop_item = Some(stop_item);
        self.prev_item = Some(prev_item);
        self.next_item = Some(next_item);
        self.watch_item = Some(watch_item);
        self.autoread_item = Some(autoread_item);
        self.quit_item = Some(quit_item);
        self.tray = Some(tray);
        if let Some(err) = self.startup_error.take() {
            self.show_error(&err);
        }
        Ok(())
    }

    fn set_status(&mut self, text: &str) {
        self.showing_error = false;
        if let Some(status) = &self.status_item {
            status.set_text(truncate_status(text));
        }
    }

    /// Show an error in the status line and print the full text to stderr.
    fn show_error(&mut self, message: &str) {
        eprintln!("Error: {message}");
        self.showing_error = true;
        if let Some(status) = &self.status_item {
            status.set_text(truncate_status(&format!("Error: {message}")));
        }
    }

    /// Enable the transport items only while reading or paused; the
    /// pause item's label follows the state.
    fn update_transport(&self) {
        if let Some(pause) = &self.pause_item {
            pause.set_text(if self.paused { "Resume" } else { "Pause" });
            pause.set_enabled(self.active);
        }
        for item in [&self.stop_item, &self.prev_item, &self.next_item]
            .into_iter()
            .flatten()
        {
            item.set_enabled(self.active);
        }
    }

    /// Switch between the plain and "new copy" badge icons.
    fn set_badged(&mut self, badged: bool) {
        if badged == self.badged {
            return;
        }
        self.badged = badged;
        let Some(tray) = &self.tray else {
            return;
        };
        let bytes = if badged {
            icon::speaker_badged()
        } else {
            icon::speaker()
        };
        match tray_icon(&bytes) {
            Some(icon) => {
                #[cfg(target_os = "macos")]
                let result = tray.set_icon_templated(Some(icon));
                #[cfg(not(target_os = "macos"))]
                let result = tray.set_icon(Some(icon));
                if let Err(err) = result {
                    eprintln!("Error: Couldn't switch the tray icon: {err}.");
                }
            }
            None => eprintln!("Error: Couldn't switch the tray icon: invalid image data."),
        }
    }

    /// Read the clipboard aloud (left click or the menu item).
    fn read_clipboard(&mut self) {
        self.set_badged(false);
        let text = match clipboard::read_text() {
            Ok(Some(text)) if text.chars().count() >= MIN_TEXT_LEN => text,
            Ok(Some(_)) => {
                self.set_status("Nothing to read: the clipboard text is too short.");
                return;
            }
            Ok(None) => {
                self.set_status("Nothing to read: the clipboard has no text.");
                return;
            }
            Err(err) => {
                self.show_error(&err.to_string());
                return;
            }
        };
        self.play_text(text);
    }

    fn play_text(&mut self, text: String) {
        if !models::is_installed(&self.models_root, self.model) {
            // Download on a worker thread, then play; a newer request while
            // a download runs replaces the pending text.
            self.pending_text = Some(text);
            if !self.downloading {
                self.start_download();
            }
            return;
        }
        if let Err(message) = self.ensure_player() {
            self.show_error(&message);
            return;
        }
        if let Some(player) = &self.player {
            player.play(text);
        }
    }

    /// Download the model on a worker thread, reporting progress into the
    /// loop at most ~2×/s for the status line.
    fn start_download(&mut self) {
        self.downloading = true;
        self.set_status("Downloading voice…");
        let proxy = self.proxy.clone();
        let root = self.models_root.clone();
        let variant = self.model;
        std::thread::spawn(move || {
            let mut last_report: Option<Instant> = None;
            let result = models::download(&root, variant, |done, total| {
                let now = Instant::now();
                let due = last_report.is_none_or(|last| now - last >= Duration::from_millis(500));
                if due {
                    last_report = Some(now);
                    let _ = proxy.send_event(AppEvent::DownloadProgress(done, total));
                }
            });
            let _ = proxy.send_event(AppEvent::DownloadDone(
                result.map(|_| ()).map_err(|err| err.to_string()),
            ));
        });
    }

    fn on_download_progress(&mut self, done: u64, total: Option<u64>) {
        self.set_status(&download_status(done, total));
    }

    fn on_download_done(&mut self, result: Result<(), String>) {
        self.downloading = false;
        match result {
            Ok(()) => {
                if let Some(text) = self.pending_text.take() {
                    self.play_text(text);
                } else {
                    self.set_status("Ready");
                }
            }
            Err(message) => {
                self.pending_text = None;
                self.show_error(&message);
            }
        }
    }

    /// Create the audio sink and player on first use (and retry after a
    /// failure, e.g. no audio device at startup).
    fn ensure_player(&mut self) -> Result<(), String> {
        if self.player.is_some() {
            return Ok(());
        }
        let sink = AudioSink::new().map_err(|err| err.to_string())?;
        let unload_after = if self.settings.unload_after_minutes == 0 {
            None
        } else {
            Some(Duration::from_secs(
                u64::from(self.settings.unload_after_minutes) * 60,
            ))
        };
        let config = PlayerConfig {
            voice: self.settings.voice.clone(),
            speed: self.settings.speed,
            unload_after,
            sanitizer_options: SanitizerOptions::default(),
            pronunciations: self.pronunciations.clone(),
        };
        let models_root = self.models_root.clone();
        let model = self.model;
        let factory = move || -> Result<Box<dyn Synth>, String> {
            match KokoroEngine::load(&models_root, model) {
                Ok(engine) => Ok(Box::new(engine) as Box<dyn Synth>),
                Err(err) => Err(err.to_string()),
            }
        };
        let (player, events) = Player::spawn(config, Box::new(factory), sink);
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            for event in events {
                if proxy.send_event(AppEvent::Player(event)).is_err() {
                    break;
                }
            }
        });
        self.player = Some(player);
        Ok(())
    }

    fn on_player_event(&mut self, event: PlayerEvent) {
        match event {
            PlayerEvent::State(PlayerState::Loading) => {
                self.active = true;
                self.last_chunk = None;
                self.paused = false;
                self.set_status("Loading voice…");
            }
            PlayerEvent::State(PlayerState::Playing) => {
                // A fresh play keeps "Loading voice…" (or "Ready" when the
                // engine was already loaded) until the first chunk; a resume
                // restores the last reading position.
                if self.paused {
                    self.paused = false;
                    if let Some((index, count)) = self.last_chunk {
                        self.set_status(&format!("Reading {} of {count}", index + 1));
                    } else {
                        self.set_status("Ready");
                    }
                }
            }
            PlayerEvent::State(PlayerState::Paused) => {
                self.paused = true;
                self.set_status("Paused");
            }
            PlayerEvent::State(PlayerState::Idle) => {
                if !self.showing_error {
                    self.active = false;
                    self.last_chunk = None;
                    self.paused = false;
                    self.set_status("Ready");
                }
            }
            PlayerEvent::Chunk { index, count, .. } => {
                self.active = true;
                self.last_chunk = Some((index, count));
                self.paused = false;
                self.set_status(&format!("Reading {} of {count}", index + 1));
            }
            PlayerEvent::Finished => {
                self.active = false;
                self.last_chunk = None;
                self.paused = false;
                self.set_status("Ready");
            }
            PlayerEvent::Error(message) => {
                self.active = false;
                self.last_chunk = None;
                self.paused = false;
                self.show_error(&message);
            }
        }
        self.update_transport();
    }

    fn on_clipboard_event(&mut self, event: ClipboardEvent) {
        match event {
            ClipboardEvent::Changed { .. } => {
                // A copy made just before Watch was unchecked may still be in
                // flight; ignore it.
                if !self.settings.watch_clipboard {
                    return;
                }
                if self.settings.auto_read {
                    self.read_clipboard();
                } else {
                    self.set_badged(true);
                }
            }
            ClipboardEvent::Error(message) => {
                // The watcher thread is gone; drop it so enabling Watch
                // starts a fresh one.
                self.watcher = None;
                self.show_error(&message);
            }
        }
    }

    fn on_tray_event(&mut self, event: TrayIconEvent) {
        // Left click reads the clipboard; the menu opens on right click.
        // (On Linux/ksni a left-click activation arrives here the same way.)
        if matches!(
            event,
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            }
        ) {
            self.read_clipboard();
        }
    }

    fn on_menu_event(&mut self, event_loop: &ActiveEventLoop, event: MenuEvent) {
        let id = event.id();
        if self.read_item.as_ref().is_some_and(|item| id == item.id()) {
            self.read_clipboard();
        } else if self.pause_item.as_ref().is_some_and(|item| id == item.id()) {
            if let Some(player) = &self.player {
                if self.paused {
                    player.resume();
                } else {
                    player.pause();
                }
            }
        } else if self.stop_item.as_ref().is_some_and(|item| id == item.id()) {
            if let Some(player) = &self.player {
                player.stop();
            }
        } else if self.prev_item.as_ref().is_some_and(|item| id == item.id()) {
            if let Some(player) = &self.player {
                player.previous();
            }
        } else if self.next_item.as_ref().is_some_and(|item| id == item.id()) {
            if let Some(player) = &self.player {
                player.next();
            }
        } else if self.watch_item.as_ref().is_some_and(|item| id == item.id()) {
            self.on_watch_toggled();
        } else if self
            .autoread_item
            .as_ref()
            .is_some_and(|item| id == item.id())
        {
            let checked = self
                .autoread_item
                .as_ref()
                .is_some_and(CheckMenuItem::is_checked);
            self.settings.auto_read = checked;
            self.save_settings();
        } else if self.quit_item.as_ref().is_some_and(|item| id == item.id()) {
            if let Some(player) = &self.player {
                player.stop();
            }
            event_loop.exit();
        }
    }

    /// The Watch check already holds its new state (muda flips it on click).
    fn on_watch_toggled(&mut self) {
        let checked = self
            .watch_item
            .as_ref()
            .is_some_and(CheckMenuItem::is_checked);
        self.settings.watch_clipboard = checked;
        if checked {
            match self.start_watcher() {
                Ok(()) => {
                    if let Some(watcher) = &self.watcher {
                        watcher.set_enabled(true);
                    }
                }
                Err(message) => {
                    // Stay unchecked: the setting never changed on disk.
                    if let Some(watch) = &self.watch_item {
                        watch.set_checked(false);
                    }
                    self.settings.watch_clipboard = false;
                    self.show_error(&message);
                    return;
                }
            }
        } else if let Some(watcher) = &self.watcher {
            watcher.set_enabled(false);
        }
        self.save_settings();
    }
}

impl ApplicationHandler<AppEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // Zero CPU when idle: wake only for new events.
        event_loop.set_control_flow(ControlFlow::Wait);
        if self.tray.is_some() {
            return;
        }
        if let Err(message) = self.create_tray() {
            eprintln!("Error: {message}");
            self.exit_code = 1;
            event_loop.exit();
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            AppEvent::Player(event) => self.on_player_event(event),
            AppEvent::Clipboard(event) => self.on_clipboard_event(event),
            AppEvent::Tray(event) => self.on_tray_event(event),
            AppEvent::Menu(event) => self.on_menu_event(event_loop, event),
            AppEvent::DownloadProgress(done, total) => self.on_download_progress(done, total),
            AppEvent::DownloadDone(result) => self.on_download_done(result),
        }
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        _event: WindowEvent,
    ) {
        // The app creates no windows; this is unreachable.
    }
}

/// Build a tray icon from 32×32 RGBA bytes (`None` on invalid data).
fn tray_icon(rgba: &[u8]) -> Option<Icon> {
    Icon::from_rgba(rgba.to_vec(), icon::SIZE, icon::SIZE).ok()
}

/// Status-line text for a running model download (decimal MB).
fn download_status(done: u64, total: Option<u64>) -> String {
    const MB: f64 = 1_000_000.0;
    match total {
        Some(total) => format!(
            "Downloading voice: {:.0}/{:.0} MB",
            done as f64 / MB,
            total as f64 / MB
        ),
        None => format!("Downloading voice: {:.0} MB", done as f64 / MB),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_status_is_unchanged() {
        assert_eq!(truncate_status("Ready"), "Ready");
        assert_eq!(truncate_status(&"x".repeat(80)), "x".repeat(80));
    }

    #[test]
    fn long_status_is_cut_at_a_char_boundary() {
        let text = format!("Error: {}", "é".repeat(100));
        let cut = truncate_status(&text);
        assert_eq!(cut.chars().count(), 80);
        assert!(cut.starts_with("Error: é"));
    }

    #[test]
    fn download_status_shows_whole_mb() {
        assert_eq!(
            download_status(120_000_000, Some(350_000_000)),
            "Downloading voice: 120/350 MB"
        );
        assert_eq!(download_status(1_500_000, None), "Downloading voice: 2 MB");
    }
}
