//! Tray application: a menu-bar icon driven by the player and the
//! clipboard watcher, with an optional reading panel window. The winit event
//! loop runs with `ControlFlow::Wait` (zero CPU when idle); while the panel is
//! open it wakes only for the auto-hide timer and word-highlight changes.
//! Player, clipboard, tray and menu events are forwarded into it from small
//! forwarder threads and the tray/menu global handlers. All UI updates run on
//! the main thread.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use tray_icon::menu::{
    CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu, accelerator::Accelerator,
};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder, TrayIconEvent};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalPosition;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
#[cfg(target_os = "macos")]
use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
use winit::window::WindowId;

use crate::audio::AudioSink;
use crate::autostart;
use crate::clipboard::{self, ClipboardEvent, MIN_TEXT_LEN};
use crate::engine::{
    KokoroEngine, ModelVariant, PERSIAN_VOICES, PersianVoice, PiperEngine, VOICES,
    voice_display_name,
};
use crate::icon;
use crate::models::{self, ModelId};
use crate::pipeline::prepare;
use crate::player::{Player, PlayerConfig, PlayerEvent, PlayerState, Synth};
use crate::pronounce::{Pronunciations, pronunciations_path};
use crate::reader::model::ReaderModel;
use crate::reader::window::{PanelAction, ReaderWindow};
use crate::sanitize::SanitizerOptions;
use crate::settings::{Settings, settings_path};
use crate::shortcuts::{ShortcutAction, format_binding, parse_binding};

/// Events forwarded into the event loop from worker threads and the
/// tray/menu global handlers.
#[derive(Debug)]
enum AppEvent {
    Player(PlayerEvent),
    Clipboard(ClipboardEvent),
    Tray(TrayIconEvent),
    Menu(MenuEvent),
    Shortcut(GlobalHotKeyEvent),
    #[cfg(target_os = "linux")]
    PortalShortcut(ShortcutAction),
    #[cfg(target_os = "linux")]
    PortalError(String),
    DownloadProgress(ModelId, u64, Option<u64>),
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

/// Speed choices in the Settings menu (a custom value from the file shows no
/// check but is kept).
const SPEEDS: [f32; 6] = [0.75, 1.0, 1.25, 1.5, 1.75, 2.0];

/// Idle-unload choices in the Settings menu: `unload_after_minutes` + label.
const UNLOAD_OPTIONS: [(u32, &str); 5] = [
    (1, "After 1 minute"),
    (5, "After 5 minutes"),
    (15, "After 15 minutes"),
    (60, "After 1 hour"),
    (0, "Never"),
];

/// Menu label for a speed (`1.0` → `"1×"`, `1.25` → `"1.25×"`).
fn speed_label(speed: f32) -> String {
    format!("{speed}×")
}

/// Player idle timeout for `unload_after_minutes` (0 = keep loaded).
fn unload_duration(minutes: u32) -> Option<Duration> {
    if minutes == 0 {
        None
    } else {
        Some(Duration::from_secs(u64::from(minutes) * 60))
    }
}

fn menu_build_error(err: tray_icon::menu::Error) -> String {
    format!("Couldn't build the tray menu: {err}.")
}

/// Display-only accelerator for a menu item, from a shortcut binding. A
/// binding that doesn't parse (the hotkey error is already shown at startup)
/// or is empty (disabled) shows no accelerator.
fn menu_accelerator(binding: &str) -> Option<Accelerator> {
    if binding.trim().is_empty() {
        return None;
    }
    binding.parse().ok()
}

/// OS default-editor opener for a file (no shell string).
fn open_command(path: &Path) -> Command {
    #[cfg(target_os = "macos")]
    {
        let mut command = Command::new("open");
        command.arg("-t").arg(path);
        command
    }
    #[cfg(target_os = "windows")]
    {
        let mut command = Command::new("cmd");
        command.arg("/C").arg("start").arg("").arg(path);
        command
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let mut command = Command::new("xdg-open");
        command.arg(path);
        command
    }
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
    let shortcut_proxy = proxy.clone();
    GlobalHotKeyEvent::set_event_handler(Some(move |event| {
        let _ = shortcut_proxy.send_event(AppEvent::Shortcut(event));
    }));

    // Checked once at startup; the latency probe is zero-cost when off.
    let debug_latency = std::env::var("SMART_VOICES_DEBUG").is_ok_and(|v| v == "1");
    let mut app = App::new(
        proxy,
        settings,
        settings_file,
        pronunciations,
        models_root,
        debug_latency,
    );
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
    launch_at_login_item: Option<CheckMenuItem>,
    watch_item: Option<CheckMenuItem>,
    autoread_item: Option<CheckMenuItem>,
    voice_items: Vec<(String, CheckMenuItem)>,
    persian_voice_items: Vec<(PersianVoice, CheckMenuItem)>,
    speed_items: Vec<(f32, CheckMenuItem)>,
    model_items: Vec<(ModelVariant, CheckMenuItem)>,
    unload_items: Vec<(u32, CheckMenuItem)>,
    edit_pron_item: Option<MenuItem>,
    open_settings_item: Option<MenuItem>,
    change_shortcuts_item: Option<MenuItem>,
    quit_item: Option<MenuItem>,
    show_reader_item: Option<CheckMenuItem>,
    open_reader_item: Option<MenuItem>,
    /// Reading panel state, driven by every player event.
    reader_model: ReaderModel,
    reader: Option<ReaderWindow>,
    /// When to auto-hide the panel after the reading ended.
    reader_hide_at: Option<Instant>,
    /// Opened from the menu: never auto-hide.
    reader_pinned: bool,
    /// Opening failed for this reading; don't retry it.
    reader_failed_reading: Option<u64>,
    /// Last panel position, reused on reopen.
    reader_position: Option<PhysicalPosition<i32>>,
    /// Global shortcut manager (must stay alive; dropping it unregisters).
    hotkey_manager: Option<GlobalHotKeyManager>,
    /// Registered hotkey id → action.
    hotkeys: Vec<(u32, ShortcutAction)>,
    /// Reading or paused (transport items enabled).
    active: bool,
    /// Last chunk position, to restore "Reading i of n" on resume.
    last_chunk: Option<(usize, usize)>,
    paused: bool,
    /// Whether the status line currently shows an error (kept until the next
    /// action; the `Idle` state right after an `Error` must not clear it).
    showing_error: bool,
    /// A model download is running on a worker thread.
    downloading: bool,
    /// Text to play once the running download finishes.
    pending_text: Option<String>,
    exit_code: i32,
    /// Latency probe (`SMART_VOICES_DEBUG=1`, checked once at startup).
    debug_latency: bool,
    /// When the last copy event arrived (only when debugging latency).
    last_copy_time: Option<Instant>,
    /// Copy→read time for the reading awaiting its first audio.
    pending_copy_to_read_ms: Option<u64>,
}

impl App {
    fn new(
        proxy: EventLoopProxy<AppEvent>,
        settings: Settings,
        settings_path: Option<PathBuf>,
        pronunciations: Pronunciations,
        models_root: PathBuf,
        debug_latency: bool,
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
            launch_at_login_item: None,
            watch_item: None,
            autoread_item: None,
            voice_items: Vec::new(),
            persian_voice_items: Vec::new(),
            speed_items: Vec::new(),
            model_items: Vec::new(),
            unload_items: Vec::new(),
            edit_pron_item: None,
            open_settings_item: None,
            change_shortcuts_item: None,
            quit_item: None,
            show_reader_item: None,
            open_reader_item: None,
            reader_model: ReaderModel::new(),
            reader: None,
            reader_hide_at: None,
            reader_pinned: false,
            reader_failed_reading: None,
            reader_position: None,
            hotkey_manager: None,
            hotkeys: Vec::new(),
            active: false,
            last_chunk: None,
            paused: false,
            showing_error: false,
            downloading: false,
            pending_text: None,
            exit_code: 0,
            debug_latency,
            last_copy_time: None,
            pending_copy_to_read_ms: None,
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
        let shortcuts = &self.settings.shortcuts;
        let status_item = MenuItem::new("Ready", false, None);
        let read_item = MenuItem::new(
            "Read Clipboard",
            true,
            menu_accelerator(&shortcuts.read_or_pause),
        );
        let pause_item = MenuItem::new("Pause", false, menu_accelerator(&shortcuts.read_or_pause));
        let stop_item = MenuItem::new("Stop", false, menu_accelerator(&shortcuts.stop));
        let prev_item = MenuItem::new(
            "Previous Sentence",
            false,
            menu_accelerator(&shortcuts.previous),
        );
        let next_item = MenuItem::new("Next Sentence", false, menu_accelerator(&shortcuts.next));
        let open_reader_item = MenuItem::new("Show Reading Panel", true, None);
        // The real OS state, read fresh (not a stored setting).
        let launch_at_login_item =
            CheckMenuItem::new("Launch at Login", true, autostart::is_enabled(), None);
        let watch_item = CheckMenuItem::new(
            "Watch the Clipboard",
            true,
            self.settings.watch_clipboard,
            None,
        );
        let show_reader_item = CheckMenuItem::new(
            "Show Reading Panel While Reading",
            true,
            self.settings.show_reader,
            None,
        );
        let autoread_item = CheckMenuItem::new(
            "Read Copied Text Automatically",
            self.settings.watch_clipboard,
            self.settings.auto_read,
            None,
        );
        let quit_item = MenuItem::new("Quit Smart Voices", true, None);

        let voice_menu = Submenu::new("Voice", true);
        let mut voice_items = Vec::with_capacity(VOICES.len());
        for id in VOICES {
            let label = voice_display_name(id).unwrap_or_else(|| id.to_string());
            let item = CheckMenuItem::new(label, true, id == self.settings.voice, None);
            voice_menu.append(&item).map_err(menu_build_error)?;
            voice_items.push((id.to_string(), item));
        }

        let persian_voice_menu = Submenu::new("Persian Voice", true);
        let mut persian_voice_items = Vec::with_capacity(PERSIAN_VOICES.len());
        for voice in PERSIAN_VOICES {
            let item = CheckMenuItem::new(
                voice.label(),
                true,
                voice == self.settings.persian_voice,
                None,
            );
            persian_voice_menu.append(&item).map_err(menu_build_error)?;
            persian_voice_items.push((voice, item));
        }

        let speed_menu = Submenu::new("Speed", true);
        let mut speed_items = Vec::with_capacity(SPEEDS.len());
        for speed in SPEEDS {
            let item =
                CheckMenuItem::new(speed_label(speed), true, speed == self.settings.speed, None);
            speed_menu.append(&item).map_err(menu_build_error)?;
            speed_items.push((speed, item));
        }

        let model_menu = Submenu::new("Voice Model", true);
        let mut model_items = Vec::with_capacity(2);
        for variant in [ModelVariant::Fast, ModelVariant::Small] {
            let item = CheckMenuItem::new(variant.label(), true, variant == self.model, None);
            model_menu.append(&item).map_err(menu_build_error)?;
            model_items.push((variant, item));
        }

        let unload_menu = Submenu::new("Unload Voice When Idle", true);
        let mut unload_items = Vec::with_capacity(UNLOAD_OPTIONS.len());
        for (minutes, label) in UNLOAD_OPTIONS {
            let item = CheckMenuItem::new(
                label,
                true,
                minutes == self.settings.unload_after_minutes,
                None,
            );
            unload_menu.append(&item).map_err(menu_build_error)?;
            unload_items.push((minutes, item));
        }

        let edit_pron_item = MenuItem::new("Edit Pronunciations…", true, None);
        let open_settings_item = MenuItem::new("Open Settings File…", true, None);

        let shortcuts_menu = Submenu::new("Shortcuts", true);
        for label in [
            format!("Read / Pause: {}", format_binding(&shortcuts.read_or_pause)),
            format!("Stop: {}", format_binding(&shortcuts.stop)),
            format!("Next Sentence: {}", format_binding(&shortcuts.next)),
            format!("Previous Sentence: {}", format_binding(&shortcuts.previous)),
        ] {
            shortcuts_menu
                .append(&MenuItem::new(label, false, None))
                .map_err(menu_build_error)?;
        }
        let change_shortcuts_item = MenuItem::new("Change Shortcuts…", true, None);
        shortcuts_menu
            .append(&change_shortcuts_item)
            .map_err(menu_build_error)?;
        shortcuts_menu
            .append(&MenuItem::new(
                "Edit settings.toml, then restart to apply",
                false,
                None,
            ))
            .map_err(menu_build_error)?;

        let settings_menu = Submenu::new("Settings", true);
        settings_menu
            .append(&launch_at_login_item)
            .map_err(menu_build_error)?;
        settings_menu
            .append(&watch_item)
            .map_err(menu_build_error)?;
        settings_menu
            .append(&show_reader_item)
            .map_err(menu_build_error)?;
        settings_menu
            .append(&voice_menu)
            .map_err(menu_build_error)?;
        settings_menu
            .append(&persian_voice_menu)
            .map_err(menu_build_error)?;
        settings_menu
            .append(&speed_menu)
            .map_err(menu_build_error)?;
        settings_menu
            .append(&model_menu)
            .map_err(menu_build_error)?;
        settings_menu
            .append(&unload_menu)
            .map_err(menu_build_error)?;
        settings_menu
            .append(&PredefinedMenuItem::separator())
            .map_err(menu_build_error)?;
        settings_menu
            .append(&edit_pron_item)
            .map_err(menu_build_error)?;
        settings_menu
            .append(&open_settings_item)
            .map_err(menu_build_error)?;
        settings_menu
            .append(&shortcuts_menu)
            .map_err(menu_build_error)?;

        menu.append(&status_item).map_err(menu_build_error)?;
        menu.append(&autoread_item).map_err(menu_build_error)?;
        menu.append(&PredefinedMenuItem::separator())
            .map_err(menu_build_error)?;
        menu.append(&read_item).map_err(menu_build_error)?;
        menu.append(&pause_item).map_err(menu_build_error)?;
        menu.append(&stop_item).map_err(menu_build_error)?;
        menu.append(&prev_item).map_err(menu_build_error)?;
        menu.append(&next_item).map_err(menu_build_error)?;
        menu.append(&open_reader_item).map_err(menu_build_error)?;
        menu.append(&PredefinedMenuItem::separator())
            .map_err(menu_build_error)?;
        menu.append(&settings_menu).map_err(menu_build_error)?;
        menu.append(&PredefinedMenuItem::separator())
            .map_err(menu_build_error)?;
        menu.append(&quit_item).map_err(menu_build_error)?;
        let icon = tray_icon(&icon::speaker())
            .ok_or_else(|| "Couldn't build the tray icon: invalid image data.".to_string())?;
        let builder = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Smart Voices")
            .with_menu_on_left_click(true);
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
        self.launch_at_login_item = Some(launch_at_login_item);
        self.watch_item = Some(watch_item);
        self.autoread_item = Some(autoread_item);
        self.voice_items = voice_items;
        self.persian_voice_items = persian_voice_items;
        self.speed_items = speed_items;
        self.model_items = model_items;
        self.unload_items = unload_items;
        self.edit_pron_item = Some(edit_pron_item);
        self.open_settings_item = Some(open_settings_item);
        self.change_shortcuts_item = Some(change_shortcuts_item);
        self.quit_item = Some(quit_item);
        self.show_reader_item = Some(show_reader_item);
        self.open_reader_item = Some(open_reader_item);
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

    /// Read the clipboard aloud (the menu item or a shortcut).
    fn read_clipboard(&mut self) {
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
        self.reload_pronunciations();
        // Download whichever models this reading needs (English chunks need
        // the Kokoro variant, Persian chunks the Persian voice), one after the
        // other, then play. English-only text never downloads the Persian
        // voice.
        let chunks = prepare(&text, &SanitizerOptions::default(), &self.pronunciations);
        let missing: Vec<ModelId> =
            models::models_for_chunks(&chunks, self.model, self.settings.persian_voice)
                .into_iter()
                .filter(|model| !models::is_installed(&self.models_root, *model))
                .collect();
        if !missing.is_empty() {
            // Download on a worker thread, then play; a newer request while
            // a download runs replaces the pending text (and the next check
            // re-computes what is still missing).
            self.pending_text = Some(text);
            if !self.downloading {
                self.start_download(missing);
            }
            return;
        }
        if let Err(message) = self.ensure_player() {
            self.show_error(&message);
            return;
        }
        if let Some(player) = &self.player {
            if self.debug_latency {
                let copy_ms = self
                    .last_copy_time
                    .map(|start| start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64)
                    .unwrap_or(0);
                self.pending_copy_to_read_ms = Some(copy_ms);
                self.last_copy_time = None;
            }
            player.play(text);
        }
    }

    /// Download the missing models on a worker thread, one after the other,
    /// reporting progress into the loop at most ~2×/s for the status line.
    fn start_download(&mut self, queue: Vec<ModelId>) {
        self.downloading = true;
        self.set_status("Downloading voice…");
        let proxy = self.proxy.clone();
        let root = self.models_root.clone();
        std::thread::spawn(move || {
            let mut last_report: Option<Instant> = None;
            for model in queue {
                let result = models::download(&root, model, |done, total| {
                    let now = Instant::now();
                    let due =
                        last_report.is_none_or(|last| now - last >= Duration::from_millis(500));
                    if due {
                        last_report = Some(now);
                        let _ = proxy.send_event(AppEvent::DownloadProgress(model, done, total));
                    }
                });
                if let Err(err) = result {
                    let _ = proxy.send_event(AppEvent::DownloadDone(Err(err.to_string())));
                    return;
                }
            }
            let _ = proxy.send_event(AppEvent::DownloadDone(Ok(())));
        });
    }

    fn on_download_progress(&mut self, model: ModelId, done: u64, total: Option<u64>) {
        self.set_status(&download_status(download_label(model), done, total));
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

    /// Re-read the pronunciations file at the start of every reading so
    /// edits apply without restarting. An invalid file shows a status-line
    /// error and the built-in list is used.
    fn reload_pronunciations(&mut self) {
        let Some(path) = pronunciations_path() else {
            return;
        };
        let (dict, err) = Pronunciations::load(&path);
        if let Some(err) = err {
            self.show_error(&err.to_string());
        }
        self.pronunciations = dict.clone();
        // Queued before `play`, so it applies from the first new chunk.
        if let Some(player) = &self.player {
            player.set_pronunciations(dict);
        }
    }

    /// Create the audio sink and player on first use (and retry after a
    /// failure, e.g. no audio device at startup).
    fn ensure_player(&mut self) -> Result<(), String> {
        if self.player.is_some() {
            return Ok(());
        }
        let sink = AudioSink::new().map_err(|err| err.to_string())?;
        let unload_after = unload_duration(self.settings.unload_after_minutes);
        let persian_voice = self.settings.persian_voice;
        let config = PlayerConfig {
            voice: self.settings.voice.clone(),
            speed: self.settings.speed,
            unload_after,
            sanitizer_options: SanitizerOptions::default(),
            pronunciations: self.pronunciations.clone(),
            persian_voice,
            debug_latency: self.debug_latency,
        };
        let models_root = self.models_root.clone();
        let model = self.model;
        let persian_root = models_root.clone();
        let factory = move || -> Result<Box<dyn Synth>, String> {
            match KokoroEngine::load(&models_root, model) {
                Ok(engine) => Ok(Box::new(engine) as Box<dyn Synth>),
                Err(err) => Err(err.to_string()),
            }
        };
        let persian_factory = move || -> Result<Box<dyn Synth>, String> {
            match PiperEngine::load(&persian_root, persian_voice) {
                Ok(engine) => Ok(Box::new(engine) as Box<dyn Synth>),
                Err(err) => Err(err.to_string()),
            }
        };
        let (player, events) =
            Player::spawn(config, Box::new(factory), Box::new(persian_factory), sink);
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

    fn on_player_event(&mut self, event_loop: &ActiveEventLoop, event: PlayerEvent) {
        self.reader_model.apply(&event, Instant::now());
        self.update_reader(event_loop);
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
            PlayerEvent::Playlist(_) => {}
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
                self.pending_copy_to_read_ms = None;
                self.show_error(&message);
            }
            PlayerEvent::Latency {
                read_to_first_ms,
                load_ms,
                synth_ms,
                chars,
            } => {
                if self.debug_latency {
                    let copy_ms = self.pending_copy_to_read_ms.take().unwrap_or(0);
                    eprintln!(
                        "latency: copy→read {copy_ms} ms, read→first audio {read_to_first_ms} ms (load {load_ms} ms, synth {synth_ms} ms, {chars} chars)"
                    );
                }
            }
        }
        self.update_transport();
    }

    /// Open the panel window if needed and sync it. A failure shows once
    /// per reading (the menu clears the guard, so opening from there retries).
    fn open_reader(&mut self, event_loop: &ActiveEventLoop) {
        if self.reader.is_none() {
            let now = Instant::now();
            match ReaderWindow::open(event_loop, &self.reader_model, now, self.reader_position) {
                Ok(window) => self.reader = Some(window),
                Err(message) => {
                    self.show_error(&message);
                    self.reader_failed_reading = Some(self.reader_model.reading_id());
                    return;
                }
            }
        }
        if let Some(reader) = &mut self.reader {
            reader.sync(&self.reader_model, Instant::now());
        }
    }

    /// Open, sync or schedule hiding the panel after a player event.
    fn update_reader(&mut self, event_loop: &ActiveEventLoop) {
        if self.reader_model.should_show(self.settings.show_reader)
            && self.reader_failed_reading != Some(self.reader_model.reading_id())
        {
            self.reader_hide_at = None;
            self.open_reader(event_loop);
        } else if self.reader.is_some() {
            let now = Instant::now();
            if let Some(reader) = &mut self.reader {
                reader.sync(&self.reader_model, now);
            }
            if !self.reader_pinned
                && !self.reader_model.is_active()
                && self.reader_hide_at.is_none()
            {
                self.reader_hide_at = Some(now + Duration::from_secs(3));
            }
        }
    }

    /// Close the panel through every path (×, hide timer, setting off, draw
    /// failure): remember the position, drop the window (freeing the fonts),
    /// and clear the hide timer and the pinned flag.
    fn close_reader(&mut self) {
        if let Some(reader) = self.reader.take() {
            self.reader_position = reader.position().or(self.reader_position);
        }
        self.reader_hide_at = None;
        self.reader_pinned = false;
    }

    /// "Show Reading Panel" menu item: show the panel even when idle, and keep
    /// it open (pinned) instead of auto-hiding it.
    fn open_reader_pinned(&mut self, event_loop: &ActiveEventLoop) {
        self.reader_model.undismiss();
        self.reader_pinned = true;
        self.reader_failed_reading = None;
        self.reader_hide_at = None;
        self.open_reader(event_loop);
    }

    /// The Settings check already holds its new state (muda flips it on click).
    fn on_show_reader_toggled(&mut self, event_loop: &ActiveEventLoop) {
        let checked = self
            .show_reader_item
            .as_ref()
            .is_some_and(CheckMenuItem::is_checked);
        self.settings.show_reader = checked;
        self.save_settings();
        if checked {
            self.update_reader(event_loop);
        } else if !self.reader_pinned {
            self.close_reader();
        }
    }

    fn on_clipboard_event(&mut self, event: ClipboardEvent) {
        match event {
            ClipboardEvent::Changed { .. } => {
                // A copy made just before Watch was unchecked may still be in
                // flight; ignore it.
                if !self.settings.watch_clipboard {
                    return;
                }
                if self.debug_latency {
                    self.last_copy_time = Some(Instant::now());
                }
                if self.settings.auto_read {
                    self.read_clipboard();
                } else {
                    // Warm the voice in the background so the manual read is
                    // fast. A failure here (e.g. no audio device) stays silent;
                    // the manual read reports it.
                    if self.ensure_player().is_ok()
                        && let Some(player) = &self.player
                    {
                        player.prewarm();
                    }
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

    fn on_tray_event(&mut self, _event: TrayIconEvent) {
        // The menu opens on either click, like other tray apps; actions run
        // from the menu items and shortcuts.
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
        } else if self
            .open_reader_item
            .as_ref()
            .is_some_and(|item| id == item.id())
        {
            self.open_reader_pinned(event_loop);
        } else if self
            .launch_at_login_item
            .as_ref()
            .is_some_and(|item| id == item.id())
        {
            self.on_launch_at_login_toggled();
        } else if self.watch_item.as_ref().is_some_and(|item| id == item.id()) {
            self.on_watch_toggled();
        } else if self
            .show_reader_item
            .as_ref()
            .is_some_and(|item| id == item.id())
        {
            self.on_show_reader_toggled(event_loop);
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
        } else if let Some(voice) = self
            .voice_items
            .iter()
            .find(|(_, item)| id == item.id())
            .map(|(voice, _)| voice.clone())
        {
            self.on_voice_selected(&voice);
        } else if let Some(voice) = self
            .persian_voice_items
            .iter()
            .find(|(_, item)| id == item.id())
            .map(|(voice, _)| *voice)
        {
            self.on_persian_voice_selected(voice);
        } else if let Some(speed) = self
            .speed_items
            .iter()
            .find(|(_, item)| id == item.id())
            .map(|(speed, _)| *speed)
        {
            self.on_speed_selected(speed);
        } else if let Some(variant) = self
            .model_items
            .iter()
            .find(|(_, item)| id == item.id())
            .map(|(variant, _)| *variant)
        {
            self.on_model_selected(variant);
        } else if let Some(minutes) = self
            .unload_items
            .iter()
            .find(|(_, item)| id == item.id())
            .map(|(minutes, _)| *minutes)
        {
            self.on_unload_selected(minutes);
        } else if self
            .edit_pron_item
            .as_ref()
            .is_some_and(|item| id == item.id())
        {
            self.open_pronunciations();
        } else if self
            .open_settings_item
            .as_ref()
            .is_some_and(|item| id == item.id())
            || self
                .change_shortcuts_item
                .as_ref()
                .is_some_and(|item| id == item.id())
        {
            // Change Shortcuts… opens the settings file too: shortcuts apply
            // on the next launch.
            self.open_settings_file();
        } else if self.quit_item.as_ref().is_some_and(|item| id == item.id()) {
            if let Some(player) = &self.player {
                player.stop();
            }
            event_loop.exit();
        }
    }

    /// The Launch at Login check already holds its new state (muda flips
    /// it on click). It mirrors the real OS state, so after a failure the
    /// check is re-synced to what the OS actually has.
    fn on_launch_at_login_toggled(&mut self) {
        let checked = self
            .launch_at_login_item
            .as_ref()
            .is_some_and(CheckMenuItem::is_checked);
        if let Err(err) = autostart::set_enabled(checked) {
            self.show_error(&err.to_string());
        }
        let enabled = autostart::is_enabled();
        if let Some(item) = &self.launch_at_login_item {
            item.set_checked(enabled);
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
        // Auto-read needs the watcher; disable the switch while off.
        if let Some(autoread) = &self.autoread_item {
            autoread.set_enabled(self.settings.watch_clipboard);
        }
        self.save_settings();
    }

    /// A voice item was clicked (muda already flipped its check; re-sync all
    /// of them so exactly one stays checked).
    fn on_voice_selected(&mut self, voice: &str) {
        self.settings.voice = voice.to_string();
        if let Some(player) = &self.player {
            player.set_voice(voice);
        }
        self.update_voice_checks();
        self.save_settings();
    }

    fn update_voice_checks(&self) {
        for (voice, item) in &self.voice_items {
            item.set_checked(*voice == self.settings.voice);
        }
    }

    /// A Persian voice item was clicked (muda already flipped its check;
    /// re-sync all of them so exactly one stays checked). Applies from the
    /// next chunk; if its model isn't installed it downloads on the next
    /// Persian reading (existing flow).
    fn on_persian_voice_selected(&mut self, voice: PersianVoice) {
        self.settings.persian_voice = voice;
        if let Some(player) = &self.player {
            player.set_persian_voice(voice);
        }
        self.update_persian_voice_checks();
        self.save_settings();
    }

    fn update_persian_voice_checks(&self) {
        for (voice, item) in &self.persian_voice_items {
            item.set_checked(*voice == self.settings.persian_voice);
        }
    }

    fn on_speed_selected(&mut self, speed: f32) {
        self.settings.speed = speed;
        if let Some(player) = &self.player {
            player.set_speed(speed);
        }
        self.update_speed_checks();
        self.save_settings();
    }

    fn update_speed_checks(&self) {
        for (speed, item) in &self.speed_items {
            item.set_checked(*speed == self.settings.speed);
        }
    }

    fn on_model_selected(&mut self, variant: ModelVariant) {
        if variant != self.model {
            if let Some(player) = &self.player {
                player.stop();
            }
            // Dropping the player shuts down its controller thread, freeing
            // the old engine; the new variant loads on the next play (and
            // downloads first if it isn't installed, via the existing flow).
            self.player = None;
            self.model = variant;
            self.settings.model = variant;
            self.active = false;
            self.last_chunk = None;
            self.paused = false;
            self.set_status("Ready");
            self.update_transport();
            self.save_settings();
        }
        self.update_model_checks();
    }

    fn update_model_checks(&self) {
        for (variant, item) in &self.model_items {
            item.set_checked(*variant == self.model);
        }
    }

    fn on_unload_selected(&mut self, minutes: u32) {
        self.settings.unload_after_minutes = minutes;
        if let Some(player) = &self.player {
            player.set_unload_after(unload_duration(minutes));
        }
        self.update_unload_checks();
        self.save_settings();
    }

    fn update_unload_checks(&self) {
        for (minutes, item) in &self.unload_items {
            item.set_checked(*minutes == self.settings.unload_after_minutes);
        }
    }

    /// Open `pronunciations.toml` in the OS default editor, creating it first
    /// if it doesn't exist yet.
    fn open_pronunciations(&mut self) {
        let Some(path) = pronunciations_path() else {
            self.show_error(
                "Couldn't open the pronunciations file: no settings path is available.",
            );
            return;
        };
        if !path.exists()
            && let Err(err) = self.pronunciations.save(&path)
        {
            self.show_error(&err.to_string());
            return;
        }
        self.open_in_editor(&path);
    }

    /// Open `settings.toml` in the OS default editor, writing the current
    /// settings first if the file is missing.
    fn open_settings_file(&mut self) {
        let Some(path) = self.settings_path.clone() else {
            self.show_error("Couldn't open the settings file: no settings path is available.");
            return;
        };
        if !path.exists()
            && let Err(err) = self.settings.save(&path)
        {
            self.show_error(&err.to_string());
            return;
        }
        self.open_in_editor(&path);
    }

    fn open_in_editor(&mut self, path: &Path) {
        if let Err(err) = open_command(path).spawn() {
            self.show_error(&format!("Couldn't open {}: {err}", path.display()));
        }
    }

    /// Register the four global shortcuts from settings. Runs once on the
    /// main thread. A binding that fails to parse or register (empty = off,
    /// taken by another app) shows an error but never stops the app or the
    /// other shortcuts.
    fn register_shortcuts(&mut self) {
        // On Wayland, global-hotkey's X11 grab can't see keys from native
        // Wayland apps; bind through the XDG GlobalShortcuts portal instead.
        #[cfg(target_os = "linux")]
        if std::env::var("WAYLAND_DISPLAY").is_ok() {
            self.register_portal_shortcuts();
            return;
        }
        let manager = match GlobalHotKeyManager::new() {
            Ok(manager) => manager,
            Err(err) => {
                self.show_error(&format!("Global shortcuts couldn't be set up: {err}"));
                return;
            }
        };
        let bindings = [
            (
                ShortcutAction::ReadOrPause,
                self.settings.shortcuts.read_or_pause.clone(),
            ),
            (ShortcutAction::Stop, self.settings.shortcuts.stop.clone()),
            (ShortcutAction::Next, self.settings.shortcuts.next.clone()),
            (
                ShortcutAction::Previous,
                self.settings.shortcuts.previous.clone(),
            ),
        ];
        for (action, text) in bindings {
            let hotkey = match parse_binding(&text) {
                Ok(None) => continue,
                Ok(Some(hotkey)) => hotkey,
                Err(err) => {
                    self.show_error(&format!("Shortcut \"{text}\" couldn't be set: {err}"));
                    continue;
                }
            };
            if let Err(err) = manager.register(hotkey) {
                self.show_error(&format!("Shortcut \"{text}\" couldn't be set: {err}"));
                continue;
            }
            self.hotkeys.push((hotkey.id(), action));
        }
        self.hotkey_manager = Some(manager);
    }

    /// Bind the four shortcuts through the XDG portal on a dedicated
    /// thread (Linux/Wayland only); activations and failures come back as
    /// loop events and run the same actions as the X11 path.
    #[cfg(target_os = "linux")]
    fn register_portal_shortcuts(&mut self) {
        let shortcuts = self.settings.shortcuts.clone();
        let proxy = self.proxy.clone();
        let activations = proxy.clone();
        std::thread::spawn(move || {
            if let Err(message) = crate::shortcuts::portal::run(&shortcuts, move |action| {
                let _ = activations.send_event(AppEvent::PortalShortcut(action));
            }) {
                let _ = proxy.send_event(AppEvent::PortalError(message));
            }
        });
    }

    fn on_shortcut(&mut self, event: GlobalHotKeyEvent) {
        if event.state() != HotKeyState::Pressed {
            return;
        }
        let action = self
            .hotkeys
            .iter()
            .find(|(id, _)| *id == event.id())
            .map(|(_, action)| *action);
        if let Some(action) = action {
            self.on_shortcut_action(action);
        }
    }

    fn on_shortcut_action(&mut self, action: ShortcutAction) {
        match Some(action) {
            Some(ShortcutAction::ReadOrPause) => {
                if self.paused {
                    if let Some(player) = &self.player {
                        player.resume();
                    }
                } else if self.active {
                    if let Some(player) = &self.player {
                        player.pause();
                    }
                } else {
                    self.read_clipboard();
                }
            }
            Some(ShortcutAction::Stop) => {
                if let Some(player) = &self.player {
                    player.stop();
                }
            }
            Some(ShortcutAction::Next) => {
                if let Some(player) = &self.player {
                    player.next();
                }
            }
            Some(ShortcutAction::Previous) => {
                if let Some(player) = &self.player {
                    player.previous();
                }
            }
            None => {}
        }
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
        } else {
            self.register_shortcuts();
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            AppEvent::Player(event) => self.on_player_event(event_loop, event),
            AppEvent::Clipboard(event) => self.on_clipboard_event(event),
            AppEvent::Tray(event) => self.on_tray_event(event),
            AppEvent::Menu(event) => self.on_menu_event(event_loop, event),
            AppEvent::Shortcut(event) => self.on_shortcut(event),
            #[cfg(target_os = "linux")]
            AppEvent::PortalShortcut(action) => self.on_shortcut_action(action),
            #[cfg(target_os = "linux")]
            AppEvent::PortalError(message) => self.show_error(&message),
            AppEvent::DownloadProgress(model, done, total) => {
                self.on_download_progress(model, done, total)
            }
            AppEvent::DownloadDone(result) => self.on_download_done(result),
        }
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(reader) = &mut self.reader else {
            return;
        };
        if reader.id() != window_id {
            return;
        }
        match reader.handle(&event, &self.reader_model) {
            PanelAction::None => {}
            PanelAction::Close => {
                self.reader_model.dismiss();
                self.close_reader();
            }
            PanelAction::Jump(index) => {
                if let Some(player) = &self.player {
                    player.jump(index);
                }
                reader.restore_previous_focus();
            }
            PanelAction::Previous => {
                if let Some(player) = &self.player {
                    player.previous();
                }
                reader.restore_previous_focus();
            }
            PanelAction::PlayPause => {
                if let Some(player) = &self.player {
                    if self.paused {
                        player.resume();
                    } else {
                        player.pause();
                    }
                }
                reader.restore_previous_focus();
            }
            PanelAction::Next => {
                if let Some(player) = &self.player {
                    player.next();
                }
                reader.restore_previous_focus();
            }
            PanelAction::DrawFailed(message) => {
                self.show_error(&message);
                self.reader_failed_reading = Some(self.reader_model.reading_id());
                self.close_reader();
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        if let Some(hide_at) = self.reader_hide_at
            && hide_at <= now
        {
            if self.reader.as_ref().is_some_and(ReaderWindow::is_hovered) {
                self.reader_hide_at = Some(now + Duration::from_secs(3));
            } else {
                self.close_reader();
            }
        }
        if let Some(reader) = &mut self.reader {
            reader.sync(&self.reader_model, now);
        }
        event_loop.set_control_flow(reader_control_flow(
            self.reader.is_some(),
            self.reader_hide_at,
            self.reader_model.next_redraw(now),
        ));
    }
}

/// Event-loop deadline while the panel is open: wake for the auto-hide timer
/// or the next word-highlight change, whichever comes first. With the panel
/// closed this is plain `Wait`, exactly as before (zero wakeups).
fn reader_control_flow(
    reader_open: bool,
    hide_at: Option<Instant>,
    redraw_at: Option<Instant>,
) -> ControlFlow {
    if !reader_open {
        return ControlFlow::Wait;
    }
    match (hide_at, redraw_at) {
        (None, None) => ControlFlow::Wait,
        (Some(at), None) | (None, Some(at)) => ControlFlow::WaitUntil(at),
        (Some(hide), Some(redraw)) => ControlFlow::WaitUntil(hide.min(redraw)),
    }
}

/// Build a tray icon from 32×32 RGBA bytes (`None` on invalid data).
fn tray_icon(rgba: &[u8]) -> Option<Icon> {
    Icon::from_rgba(rgba.to_vec(), icon::SIZE, icon::SIZE).ok()
}

/// Status-line label for a running model download.
fn download_label(model: ModelId) -> &'static str {
    match model {
        ModelId::Kokoro(_) => "Downloading voice",
        ModelId::Persian(_) => "Downloading Persian voice",
    }
}

/// Status-line text for a running model download (decimal MB).
fn download_status(label: &str, done: u64, total: Option<u64>) -> String {
    const MB: f64 = 1_000_000.0;
    match total {
        Some(total) => format!(
            "{label}: {:.0}/{:.0} MB",
            done as f64 / MB,
            total as f64 / MB
        ),
        None => format!("{label}: {:.0} MB", done as f64 / MB),
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
    fn speed_labels_use_multiplier_sign() {
        assert_eq!(speed_label(0.75), "0.75×");
        assert_eq!(speed_label(1.0), "1×");
        assert_eq!(speed_label(1.25), "1.25×");
        assert_eq!(speed_label(1.5), "1.5×");
        assert_eq!(speed_label(1.75), "1.75×");
        assert_eq!(speed_label(2.0), "2×");
    }

    #[test]
    fn unload_duration_maps_zero_to_keep_loaded() {
        assert_eq!(unload_duration(0), None);
        assert_eq!(unload_duration(5), Some(Duration::from_secs(300)));
        assert_eq!(unload_duration(60), Some(Duration::from_secs(3600)));
    }

    #[test]
    fn reader_deadline_picks_earlier_and_ignores_when_closed() {
        let now = Instant::now();
        let soon = now + Duration::from_secs(1);
        let later = now + Duration::from_secs(3);
        assert_eq!(reader_control_flow(false, None, None), ControlFlow::Wait);
        assert_eq!(reader_control_flow(true, None, None), ControlFlow::Wait);
        assert_eq!(
            reader_control_flow(true, Some(later), None),
            ControlFlow::WaitUntil(later)
        );
        assert_eq!(
            reader_control_flow(true, None, Some(soon)),
            ControlFlow::WaitUntil(soon)
        );
        assert_eq!(
            reader_control_flow(true, Some(later), Some(soon)),
            ControlFlow::WaitUntil(soon)
        );
        assert_eq!(
            reader_control_flow(true, Some(soon), Some(later)),
            ControlFlow::WaitUntil(soon)
        );
        // Closed: no wakeups even with deadlines pending.
        assert_eq!(
            reader_control_flow(false, Some(later), Some(soon)),
            ControlFlow::Wait
        );
    }

    #[test]
    fn default_bindings_show_accelerators_and_garbage_shows_none() {
        use crate::settings::Shortcuts;
        let defaults = Shortcuts::default();
        for binding in [
            &defaults.read_or_pause,
            &defaults.stop,
            &defaults.next,
            &defaults.previous,
        ] {
            assert!(menu_accelerator(binding).is_some(), "{binding}");
        }
        assert_eq!(menu_accelerator(""), None);
        assert_eq!(menu_accelerator("ctrl+Space+alt"), None);
    }

    #[test]
    fn download_status_shows_whole_mb() {
        assert_eq!(
            download_status("Downloading voice", 120_000_000, Some(350_000_000)),
            "Downloading voice: 120/350 MB"
        );
        assert_eq!(
            download_status("Downloading voice", 1_500_000, None),
            "Downloading voice: 2 MB"
        );
        assert_eq!(
            download_status(
                download_label(ModelId::Persian(crate::engine::PersianVoice::Amir)),
                30_000_000,
                Some(67_000_000)
            ),
            "Downloading Persian voice: 30/67 MB"
        );
        assert_eq!(
            download_label(ModelId::Kokoro(crate::engine::ModelVariant::Fast)),
            "Downloading voice"
        );
    }
}
