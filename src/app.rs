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
    CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu,
    accelerator::Accelerator,
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
    ModelVariant, PERSIAN_VOICES, PersianVoice, VOICES, grouped_english_voices, voice_short_name,
};
use crate::icon;
use crate::models::{self, ModelId};
use crate::phonon::{DEFAULT_PHONON_VOICE, PHONON_VOICES, PhononVoiceGroup};
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

/// Time between tray-icon animation frames while reading.
const ICON_FRAME_PERIOD: Duration = Duration::from_millis(400);

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
        app.startup_error = Some(match app.startup_error.take() {
            Some(existing) => format!("{existing}\n{message}"),
            None => message,
        });
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
    /// Prebuilt tray icons: the idle icon plus the 3 reading frames.
    tray_idle_icon: Option<Icon>,
    tray_speaking_icons: Vec<Icon>,
    /// Shown reading frame (`None` = idle icon).
    tray_frame: Option<u8>,
    /// When to advance to the next reading frame (`None` = not animating).
    icon_next_at: Option<Instant>,
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
    /// Disabled Kokoro group headers in `VoiceGroup::ALL` order (kept so the
    /// Kokoro/Phonon voice sets can be swapped).
    voice_headers: Vec<MenuItem>,
    /// The "English Voice" submenu handle (for the voice-set swap).
    english_voice_menu: Option<Submenu>,
    /// Phonon voice checks: the `"Default"` item first, then every catalog
    /// voice (`voice.id`, labeled `voice.name`).
    phonon_voice_items: Vec<(&'static str, CheckMenuItem)>,
    /// Disabled Phonon group headers in `PhononVoiceGroup::ALL` order.
    phonon_voice_headers: Vec<MenuItem>,
    /// Whether the Phonon voice set is currently shown in the submenu.
    phonon_voices_shown: bool,
    persian_voice_items: Vec<(PersianVoice, CheckMenuItem)>,
    speed_items: Vec<(f32, CheckMenuItem)>,
    model_items: Vec<(ModelVariant, CheckMenuItem)>,
    unload_items: Vec<(u32, CheckMenuItem)>,
    edit_pron_item: Option<MenuItem>,
    open_settings_item: Option<MenuItem>,
    change_shortcuts_item: Option<MenuItem>,
    quit_item: Option<MenuItem>,
    show_reader_item: Option<CheckMenuItem>,
    animate_icon_item: Option<CheckMenuItem>,
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
        let mut settings = settings;
        // An unknown saved id must not fail silently: warn loudly and use
        // the default (shown in the status line once the menu exists).
        let mut startup_error = None;
        if !crate::phonon::is_known_voice(&settings.phonon_voice) {
            let message = format!(
                "Unknown Phonon voice '{}' in settings; using the default Phonon voice.",
                settings.phonon_voice
            );
            eprintln!("Warning: {message}");
            startup_error = Some(message);
            settings.phonon_voice = crate::phonon::DEFAULT_PHONON_VOICE.to_string();
        }
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
            startup_error,
            tray: None,
            tray_idle_icon: None,
            tray_speaking_icons: Vec::new(),
            tray_frame: None,
            icon_next_at: None,
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
            voice_headers: Vec::new(),
            english_voice_menu: None,
            phonon_voice_items: Vec::new(),
            phonon_voice_headers: Vec::new(),
            phonon_voices_shown: false,
            persian_voice_items: Vec::new(),
            speed_items: Vec::new(),
            model_items: Vec::new(),
            unload_items: Vec::new(),
            edit_pron_item: None,
            open_settings_item: None,
            change_shortcuts_item: None,
            quit_item: None,
            show_reader_item: None,
            animate_icon_item: None,
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
        let animate_icon_item = CheckMenuItem::new(
            "Animate Icon While Reading",
            true,
            self.settings.animate_icon,
            None,
        );
        let autoread_item = CheckMenuItem::new(
            "Read Copied Text Automatically",
            self.settings.watch_clipboard,
            self.settings.auto_read,
            None,
        );
        let quit_item = MenuItem::new("Quit Smart Voices", true, None);

        let model_menu = Submenu::new("Voice Model", true);
        let mut model_items = Vec::with_capacity(3);
        for variant in [
            ModelVariant::Fast,
            ModelVariant::Small,
            ModelVariant::Phonon,
        ] {
            let item = CheckMenuItem::new(variant.label(), true, variant == self.model, None);
            model_menu.append(&item).map_err(menu_build_error)?;
            model_items.push((variant, item));
        }

        let english_voice_menu = Submenu::new("English Voice", true);
        // Both voice sets are built; only the selected model's set is
        // appended (the other stays stored for the swap in
        // `update_english_voice_state`).
        let mut voice_headers = Vec::with_capacity(4);
        let mut voice_items = Vec::with_capacity(VOICES.len());
        for (group, ids) in grouped_english_voices() {
            voice_headers.push(MenuItem::new(group.header(), false, None));
            for id in ids {
                let name = voice_short_name(id).unwrap_or_else(|| id.to_string());
                let item = CheckMenuItem::new(name, true, id == self.settings.voice, None);
                voice_items.push((id.to_string(), item));
            }
        }
        let mut phonon_voice_headers = Vec::with_capacity(PhononVoiceGroup::ALL.len());
        let mut phonon_voice_items: Vec<(&'static str, CheckMenuItem)> =
            Vec::with_capacity(PHONON_VOICES.len() + 1);
        phonon_voice_items.push((
            DEFAULT_PHONON_VOICE,
            CheckMenuItem::new(
                "Default",
                true,
                self.settings.phonon_voice == DEFAULT_PHONON_VOICE,
                None,
            ),
        ));
        for group in PhononVoiceGroup::ALL {
            phonon_voice_headers.push(MenuItem::new(group.header(), false, None));
            for voice in PHONON_VOICES.iter().filter(|voice| voice.group == group) {
                let item = CheckMenuItem::new(
                    voice.name,
                    true,
                    voice.id == self.settings.phonon_voice,
                    None,
                );
                phonon_voice_items.push((voice.id, item));
            }
        }
        let phonon_voices_shown = self.model == ModelVariant::Phonon;
        let active_entries = if phonon_voices_shown {
            phonon_voice_entries(&phonon_voice_headers, &phonon_voice_items)
        } else {
            kokoro_voice_entries(&voice_headers, &voice_items)
        };
        for entry in active_entries {
            english_voice_menu.append(entry).map_err(menu_build_error)?;
        }
        english_voice_menu
            .append(&PredefinedMenuItem::separator())
            .map_err(menu_build_error)?;
        english_voice_menu
            .append(&model_menu)
            .map_err(menu_build_error)?;

        let persian_voice_menu = Submenu::new("Persian Voice", true);
        let mut persian_voice_items = Vec::with_capacity(PERSIAN_VOICES.len());
        for voice in PERSIAN_VOICES {
            if voice.is_piper() {
                continue;
            }
            let item = CheckMenuItem::new(
                voice.label(),
                true,
                voice == self.settings.persian_voice,
                None,
            );
            persian_voice_menu.append(&item).map_err(menu_build_error)?;
            persian_voice_items.push((voice, item));
        }
        persian_voice_menu
            .append(&PredefinedMenuItem::separator())
            .map_err(menu_build_error)?;
        persian_voice_menu
            .append(&MenuItem::new("Piper (smaller, faster)", false, None))
            .map_err(menu_build_error)?;
        for voice in PERSIAN_VOICES {
            if !voice.is_piper() {
                continue;
            }
            let item = CheckMenuItem::new(
                voice.short_label(),
                true,
                voice == self.settings.persian_voice,
                None,
            );
            persian_voice_menu.append(&item).map_err(menu_build_error)?;
            persian_voice_items.push((voice, item));
        }

        let speed_menu = Submenu::new("Reading Speed", true);
        let mut speed_items = Vec::with_capacity(SPEEDS.len());
        for speed in SPEEDS {
            let item =
                CheckMenuItem::new(speed_label(speed), true, speed == self.settings.speed, None);
            speed_menu.append(&item).map_err(menu_build_error)?;
            speed_items.push((speed, item));
        }

        let unload_menu = Submenu::new("Free Memory When Idle", true);
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

        // Top-level order pinned by the `settings_menu_order` test — keep in
        // sync (tray-icon menus can't be inspected in tests).
        let settings_menu = Submenu::new("Settings", true);
        settings_menu
            .append(&english_voice_menu)
            .map_err(menu_build_error)?;
        settings_menu
            .append(&persian_voice_menu)
            .map_err(menu_build_error)?;
        settings_menu
            .append(&speed_menu)
            .map_err(menu_build_error)?;
        settings_menu
            .append(&PredefinedMenuItem::separator())
            .map_err(menu_build_error)?;
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
            .append(&animate_icon_item)
            .map_err(menu_build_error)?;
        settings_menu
            .append(&unload_menu)
            .map_err(menu_build_error)?;
        settings_menu
            .append(&PredefinedMenuItem::separator())
            .map_err(menu_build_error)?;
        settings_menu
            .append(&shortcuts_menu)
            .map_err(menu_build_error)?;
        settings_menu
            .append(&edit_pron_item)
            .map_err(menu_build_error)?;
        settings_menu
            .append(&open_settings_item)
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
        let idle_icon = tray_icon(&icon::speaker())
            .ok_or_else(|| "Couldn't build the tray icon: invalid image data.".to_string())?;
        let speaking_icons: Vec<Icon> = [1, 2, 3]
            .into_iter()
            .map(|waves| tray_icon(&icon::speaking(waves)))
            .collect::<Option<Vec<Icon>>>()
            .ok_or_else(|| "Couldn't build the tray icon: invalid image data.".to_string())?;
        self.tray_idle_icon = Some(idle_icon.clone());
        self.tray_speaking_icons = speaking_icons;
        let builder = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Smart Voices")
            .with_menu_on_left_click(true);
        // macOS draws the icon as a template so it adapts to light/dark menu
        // bars; elsewhere the white glyph suits the usual dark trays.
        #[cfg(target_os = "macos")]
        let builder = builder.with_icon_templated(idle_icon);
        #[cfg(not(target_os = "macos"))]
        let builder = builder.with_icon(idle_icon);
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
        self.voice_headers = voice_headers;
        self.english_voice_menu = Some(english_voice_menu);
        self.phonon_voice_items = phonon_voice_items;
        self.phonon_voice_headers = phonon_voice_headers;
        self.phonon_voices_shown = phonon_voices_shown;
        self.persian_voice_items = persian_voice_items;
        self.speed_items = speed_items;
        self.model_items = model_items;
        self.unload_items = unload_items;
        self.edit_pron_item = Some(edit_pron_item);
        self.open_settings_item = Some(open_settings_item);
        self.change_shortcuts_item = Some(change_shortcuts_item);
        self.quit_item = Some(quit_item);
        self.show_reader_item = Some(show_reader_item);
        self.animate_icon_item = Some(animate_icon_item);
        self.open_reader_item = Some(open_reader_item);
        self.tray = Some(tray);
        self.update_english_voice_state();
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

    /// Show tray frame `frame` (`None` = idle icon). The icon is only set
    /// when the frame actually changes. Returns false when setting the icon
    /// failed: the error is shown once and the animation is stopped (no
    /// per-frame retry).
    fn set_tray_frame(&mut self, frame: Option<u8>) -> bool {
        if self.tray_frame == frame {
            return true;
        }
        self.tray_frame = frame;
        let icon = match frame {
            None => self.tray_idle_icon.clone(),
            Some(n) => n
                .checked_sub(1)
                .and_then(|i| self.tray_speaking_icons.get(usize::from(i)))
                .cloned(),
        };
        let result = {
            let (Some(tray), Some(icon)) = (self.tray.as_ref(), icon) else {
                return true;
            };
            #[cfg(target_os = "macos")]
            {
                tray.set_icon_templated(Some(icon))
            }
            #[cfg(not(target_os = "macos"))]
            {
                tray.set_icon(Some(icon))
            }
        };
        if let Err(err) = result {
            self.icon_next_at = None;
            self.show_error(&format!("Couldn't update the tray icon: {err}"));
            return false;
        }
        true
    }

    /// Show the idle tray icon and stop the icon animation.
    fn show_idle_frame(&mut self) {
        self.icon_next_at = None;
        self.set_tray_frame(None);
    }

    /// Show the reading tray icon: animate from frame 1 when the setting is
    /// on, or static frame 3 when it's off. A running animation is left alone
    /// (later chunks must not restart it).
    fn show_reading_frame(&mut self) {
        if self.settings.animate_icon {
            if self.tray_frame.is_none() {
                self.start_icon_animation();
            }
        } else {
            self.icon_next_at = None;
            self.set_tray_frame(Some(3));
        }
    }

    /// Start the reading animation from frame 1 (no deadline when setting
    /// the icon failed).
    fn start_icon_animation(&mut self) {
        if self.set_tray_frame(Some(1)) {
            self.icon_next_at = Some(Instant::now() + ICON_FRAME_PERIOD);
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
        // the English variant, Persian chunks the Persian voice), one after the
        // other, then play. English-only text never downloads the Persian
        // voice.
        let chunks = prepare(&text, &SanitizerOptions::default(), &self.pronunciations);
        let missing: Vec<ModelId> = models::models_for_chunks(
            &chunks,
            self.model,
            self.settings.persian_voice,
            &self.settings.phonon_voice,
        )
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
        let voice = if self.model == ModelVariant::Phonon {
            self.settings.phonon_voice.clone()
        } else {
            self.settings.voice.clone()
        };
        let config = PlayerConfig {
            voice,
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
            crate::player::load_english_engine(&models_root, model)
        };
        let persian_factory = move || -> Result<Box<dyn Synth>, String> {
            crate::player::load_persian_engine(&persian_root, persian_voice)
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
                self.show_idle_frame();
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
                if self.active {
                    self.show_reading_frame();
                }
            }
            PlayerEvent::State(PlayerState::Paused) => {
                self.paused = true;
                self.set_status("Paused");
                self.show_idle_frame();
            }
            PlayerEvent::State(PlayerState::Idle) => {
                if !self.showing_error {
                    self.active = false;
                    self.last_chunk = None;
                    self.paused = false;
                    self.set_status("Ready");
                    self.show_idle_frame();
                }
            }
            PlayerEvent::Chunk { index, count, .. } => {
                self.active = true;
                self.last_chunk = Some((index, count));
                let paused = self.reader_model.state() == PlayerState::Paused;
                let (status, animate) = chunk_ui(paused);
                match status {
                    ChunkStatus::Paused => {
                        self.paused = true;
                        self.set_status("Paused");
                    }
                    ChunkStatus::Reading => {
                        self.paused = false;
                        self.set_status(&format!("Reading {} of {count}", index + 1));
                    }
                }
                if animate {
                    self.show_reading_frame();
                } else {
                    self.show_idle_frame();
                }
            }
            PlayerEvent::Playlist(_) => {}
            PlayerEvent::Finished => {
                self.active = false;
                self.last_chunk = None;
                self.paused = false;
                self.set_status("Ready");
                self.show_idle_frame();
            }
            PlayerEvent::Error(message) => {
                self.active = false;
                self.last_chunk = None;
                self.paused = false;
                self.pending_copy_to_read_ms = None;
                self.show_idle_frame();
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

    /// The Animate Icon check already holds its new state (muda flips it on
    /// click).
    fn on_animate_icon_toggled(&mut self) {
        let checked = self
            .animate_icon_item
            .as_ref()
            .is_some_and(CheckMenuItem::is_checked);
        self.settings.animate_icon = checked;
        self.save_settings();
        // A reading frame is showing: switch right away (paused keeps the
        // idle icon and picks the new mode up on resume).
        if self.tray_frame.is_some() {
            if checked {
                self.start_icon_animation();
            } else {
                self.icon_next_at = None;
                self.set_tray_frame(Some(3));
            }
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
            .animate_icon_item
            .as_ref()
            .is_some_and(|item| id == item.id())
        {
            self.on_animate_icon_toggled();
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
        } else if let Some(id) = self
            .phonon_voice_items
            .iter()
            .find(|(_, item)| id == item.id())
            .map(|(id, _)| *id)
        {
            self.on_phonon_voice_selected(id);
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

    /// A Phonon voice item was clicked (muda already flipped its check;
    /// re-sync all of them so exactly one stays checked).
    fn on_phonon_voice_selected(&mut self, id: &'static str) {
        self.settings.phonon_voice = id.to_string();
        if let Some(player) = &self.player {
            player.set_voice(id);
        }
        self.update_phonon_voice_checks();
        self.save_settings();
    }

    fn update_phonon_voice_checks(&self) {
        for (id, item) in &self.phonon_voice_items {
            item.set_checked(*id == self.settings.phonon_voice);
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
        self.update_english_voice_state();
    }

    fn update_model_checks(&self) {
        for (variant, item) in &self.model_items {
            item.set_checked(*variant == self.model);
        }
    }

    /// Sync the "English Voice" submenu with the selected model: it lists
    /// only that model's voices (Kokoro for Fast/Small, the catalog for
    /// Phonon). A change between the Kokoro family and Phonon swaps the whole
    /// set; Fast↔Small shows the same set and is a no-op.
    fn update_english_voice_state(&mut self) {
        let phonon = self.model == ModelVariant::Phonon;
        if phonon == self.phonon_voices_shown {
            return;
        }
        let result = match &self.english_voice_menu {
            Some(menu) => {
                let (old, new) = if phonon {
                    (
                        kokoro_voice_entries(&self.voice_headers, &self.voice_items),
                        phonon_voice_entries(&self.phonon_voice_headers, &self.phonon_voice_items),
                    )
                } else {
                    (
                        phonon_voice_entries(&self.phonon_voice_headers, &self.phonon_voice_items),
                        kokoro_voice_entries(&self.voice_headers, &self.voice_items),
                    )
                };
                swap_voice_sets(menu, &old, &new)
            }
            None => return,
        };
        match result {
            Ok(()) => self.phonon_voices_shown = phonon,
            Err(err) => {
                let message = format!("Couldn't update the English Voice menu: {err}");
                eprintln!("Warning: {message}");
                self.show_error(&message);
            }
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
        if let Some(next_at) = self.icon_next_at
            && next_at <= now
        {
            let next = match self.tray_frame {
                Some(1) => 2,
                Some(2) => 3,
                _ => 1,
            };
            if self.set_tray_frame(Some(next)) {
                self.icon_next_at = Some(now + ICON_FRAME_PERIOD);
            }
        }
        let redraw_at = if self.reader.is_some() {
            self.reader_model.next_redraw(now)
        } else {
            None
        };
        event_loop.set_control_flow(next_wake(&[
            self.reader_hide_at,
            redraw_at,
            self.icon_next_at,
        ]));
    }
}

/// Status kind for a `Chunk` event: a jump while paused stays "Paused",
/// otherwise the chunk reads as "Reading i of n".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChunkStatus {
    Paused,
    Reading,
}

/// Decide the `Chunk` UI update from the paused state: the status kind plus
/// whether to show the reading (animating) icon.
fn chunk_ui(paused: bool) -> (ChunkStatus, bool) {
    if paused {
        (ChunkStatus::Paused, false)
    } else {
        (ChunkStatus::Reading, true)
    }
}

/// Event-loop deadline: wake for the earliest pending deadline, or plain
/// `Wait` when there are none (zero wakeups when idle).
fn next_wake(deadlines: &[Option<Instant>]) -> ControlFlow {
    deadlines
        .iter()
        .flatten()
        .min()
        .map_or(ControlFlow::Wait, |at| ControlFlow::WaitUntil(*at))
}

/// Build a tray icon from 32×32 RGBA bytes (`None` on invalid data).
fn tray_icon(rgba: &[u8]) -> Option<Icon> {
    Icon::from_rgba(rgba.to_vec(), icon::SIZE, icon::SIZE).ok()
}

/// Ordered "English Voice" submenu entries of the Kokoro set: per group, the
/// disabled header then that group's voice checks.
fn kokoro_voice_entries<'a>(
    headers: &'a [MenuItem],
    items: &'a [(String, CheckMenuItem)],
) -> Vec<&'a dyn IsMenuItem> {
    let mut entries: Vec<&dyn IsMenuItem> = Vec::with_capacity(headers.len() + items.len());
    for ((_group, ids), header) in grouped_english_voices().into_iter().zip(headers.iter()) {
        entries.push(header);
        for id in ids {
            let (_, item) = items
                .iter()
                .find(|(voice, _)| voice == id)
                .expect("a voice item is built for every Kokoro id");
            entries.push(item);
        }
    }
    entries
}

/// Ordered "English Voice" submenu entries of the Phonon set: the "Default"
/// check first, then per group the disabled header and that group's voices.
fn phonon_voice_entries<'a>(
    headers: &'a [MenuItem],
    items: &'a [(&'static str, CheckMenuItem)],
) -> Vec<&'a dyn IsMenuItem> {
    let mut entries: Vec<&dyn IsMenuItem> = Vec::with_capacity(headers.len() + items.len());
    let (_, default) = items.first().expect("the Default item is built first");
    entries.push(default);
    for (group, header) in PhononVoiceGroup::ALL.into_iter().zip(headers.iter()) {
        entries.push(header);
        for voice in PHONON_VOICES.iter().filter(|voice| voice.group == group) {
            let (_, item) = items
                .iter()
                .find(|(id, _)| *id == voice.id)
                .expect("a voice item is built for every catalog voice");
            entries.push(item);
        }
    }
    entries
}

/// Swap the "English Voice" submenu's voice set: remove every entry of the
/// old set, then insert the new set's entries at positions 0..n, so the
/// separator and Voice Model submenu stay at the bottom.
fn swap_voice_sets(
    menu: &Submenu,
    old: &[&dyn IsMenuItem],
    new: &[&dyn IsMenuItem],
) -> Result<(), tray_icon::menu::Error> {
    for entry in old {
        menu.remove(*entry)?;
    }
    for (position, entry) in new.iter().enumerate() {
        menu.insert(*entry, position)?;
    }
    Ok(())
}

/// Status-line label for a running model download.
fn download_label(model: ModelId) -> &'static str {
    match model {
        ModelId::English(_) => "Downloading voice",
        ModelId::Persian(_) => "Downloading Persian voice",
        ModelId::PhononVoices => "Downloading Phonon voices",
    }
}

/// Top-level titles of the `Settings` submenu in display order (`None` =
/// separator), mirroring the append order in `create_tray`. The built menu
/// can't be inspected in tests, so the test below pins this list against the
/// owner-approved layout instead.
#[cfg(test)]
fn settings_menu_order() -> [Option<&'static str>; 13] {
    [
        Some("English Voice"),
        Some("Persian Voice"),
        Some("Reading Speed"),
        None,
        Some("Launch at Login"),
        Some("Watch the Clipboard"),
        Some("Show Reading Panel While Reading"),
        Some("Animate Icon While Reading"),
        Some("Free Memory When Idle"),
        None,
        Some("Shortcuts"),
        Some("Edit Pronunciations…"),
        Some("Open Settings File…"),
    ]
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
    fn chunk_ui_splits_paused_and_reading() {
        assert_eq!(chunk_ui(true), (ChunkStatus::Paused, false));
        assert_eq!(chunk_ui(false), (ChunkStatus::Reading, true));
    }

    #[test]
    fn next_wake_picks_earliest_or_waits() {
        let now = Instant::now();
        let soon = now + Duration::from_secs(1);
        let later = now + Duration::from_secs(3);
        assert_eq!(next_wake(&[]), ControlFlow::Wait);
        assert_eq!(next_wake(&[None, None, None]), ControlFlow::Wait);
        assert_eq!(
            next_wake(&[Some(later), None, None]),
            ControlFlow::WaitUntil(later)
        );
        assert_eq!(
            next_wake(&[None, Some(soon), None]),
            ControlFlow::WaitUntil(soon)
        );
        assert_eq!(
            next_wake(&[None, None, Some(soon)]),
            ControlFlow::WaitUntil(soon)
        );
        assert_eq!(
            next_wake(&[Some(later), Some(soon), None]),
            ControlFlow::WaitUntil(soon)
        );
        assert_eq!(
            next_wake(&[Some(soon), Some(later), Some(soon)]),
            ControlFlow::WaitUntil(soon)
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
    fn settings_menu_lists_voices_first_in_owner_order() {
        assert_eq!(
            settings_menu_order(),
            [
                Some("English Voice"),
                Some("Persian Voice"),
                Some("Reading Speed"),
                None,
                Some("Launch at Login"),
                Some("Watch the Clipboard"),
                Some("Show Reading Panel While Reading"),
                Some("Animate Icon While Reading"),
                Some("Free Memory When Idle"),
                None,
                Some("Shortcuts"),
                Some("Edit Pronunciations…"),
                Some("Open Settings File…"),
            ]
        );
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
            download_label(ModelId::English(crate::engine::ModelVariant::Fast)),
            "Downloading voice"
        );
    }
}
