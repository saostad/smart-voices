//! Reading panel window: a winit window painted with softbuffer + [`Painter`].
//!
//! The window floats above normal windows but never steals focus, follows the
//! system light/dark theme, and repaints only when something visible changed
//! (see [`ReaderWindow::sync`]): there is no polling and no per-frame work.

use std::num::NonZeroU32;
use std::rc::Rc;
use std::time::Instant;

use softbuffer::{Context, Surface};
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Theme as WinitTheme, Window, WindowId, WindowLevel};

use crate::player::PlayerState;
use crate::reader::model::ReaderModel;
use crate::reader::render::{Frame, Hit, Painter, Theme};
#[cfg(target_os = "macos")]
use objc2::rc::Retained;
#[cfg(target_os = "macos")]
use objc2_app_kit::{NSRunningApplication, NSWorkspace};

/// Logical window sizes.
const PANEL_W: f32 = 440.0;
const PANEL_H: f32 = 300.0;
const MIN_W: f32 = 320.0;
const MIN_H: f32 = 220.0;
/// Wheel scrolling: 3 lines of 21 logical px each, times the scale.
const WHEEL_LINES: f32 = 3.0;
const LINE_H: f32 = 21.0;

/// What the app should do after a panel event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelAction {
    None,
    Close,
    Jump(usize),
    Previous,
    PlayPause,
    Next,
    DrawFailed(String),
}

/// Everything visible in one painted frame. [`ReaderWindow::sync`] keeps the
/// last key and redraws only when it changed.
#[derive(Debug, Clone, PartialEq)]
struct FrameKey {
    reading_id: u64,
    current: Option<usize>,
    word: Option<usize>,
    label: String,
    can_previous: bool,
    can_next: bool,
    playing: bool,
    play_enabled: bool,
    scroll_bits: u32,
}

impl FrameKey {
    fn new(model: &ReaderModel, now: Instant, scroll: f32) -> Self {
        Self {
            reading_id: model.reading_id(),
            current: model.current(),
            word: model.current_word(now),
            label: model.progress_label(),
            can_previous: model.can_previous(),
            can_next: model.can_next(),
            playing: model.state() == PlayerState::Playing,
            play_enabled: model.is_active(),
            scroll_bits: scroll.to_bits(),
        }
    }
}

/// Wheel delta → content pixels (positive scrolls the content down).
fn wheel_px(delta: &MouseScrollDelta, scale: f32) -> f32 {
    match *delta {
        MouseScrollDelta::LineDelta(_, y) => -y * WHEEL_LINES * LINE_H * scale,
        MouseScrollDelta::PixelDelta(position) => -(position.y as f32),
    }
}

/// Tag the panel window as device RGB so CoreAnimation skips a per-frame P3
/// conversion of softbuffer's DeviceRGB frames. Best-effort: any failure only
/// warns, the panel still works.
#[cfg(target_os = "macos")]
fn set_device_rgb_color_space(window: &Window) {
    use objc2::rc::Retained;
    use objc2_app_kit::{NSColorSpace, NSView};
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let ns_view_ptr = match window.window_handle() {
        Ok(handle) => match handle.as_raw() {
            RawWindowHandle::AppKit(appkit) => appkit.ns_view.as_ptr() as *mut NSView,
            _ => {
                eprintln!(
                    "Warning: Couldn't set the reading panel's color space: no AppKit handle"
                );
                return;
            }
        },
        Err(err) => {
            eprintln!("Warning: Couldn't set the reading panel's color space: {err}");
            return;
        }
    };
    // SAFETY: the pointer comes from winit's live AppKit window handle on the
    // main thread and stays valid while the window lives; `retain` keeps it
    // alive for this scope.
    let Some(view) = (unsafe { Retained::retain(ns_view_ptr) }) else {
        eprintln!("Warning: Couldn't set the reading panel's color space: null view");
        return;
    };
    let Some(ns_window) = view.window() else {
        eprintln!("Warning: Couldn't set the reading panel's color space: view has no window");
        return;
    };
    ns_window.setColorSpace(Some(&NSColorSpace::deviceRGBColorSpace()));
}

/// The on-screen reading panel: window, softbuffer surface, painter, theme.
pub struct ReaderWindow {
    window: Rc<Window>,
    // Only a factory for the surface; kept for its lifetime.
    #[allow(dead_code)]
    context: Context<Rc<Window>>,
    surface: Surface<Rc<Window>, Rc<Window>>,
    painter: Painter,
    theme: Theme,
    reading_id: u64,
    current: Option<usize>,
    last_key: Option<FrameKey>,
    /// The user wheeled since the last playlist; auto-scroll stays off.
    user_scrolled: bool,
    hovered: bool,
    /// Last cursor position, for click hit-testing (`MouseInput` carries no
    /// coordinates).
    cursor: Option<PhysicalPosition<f64>>,
    position: Option<PhysicalPosition<i32>>,
    /// App that was frontmost when the cursor entered (macOS focus restore).
    #[cfg(target_os = "macos")]
    previous_app: Option<Retained<NSRunningApplication>>,
    /// Foreground window when the cursor entered (Windows focus restore).
    #[cfg(target_os = "windows")]
    previous_window: Option<windows_sys::Win32::Foundation::HWND>,
}

impl ReaderWindow {
    /// Open the panel. `position` reuses the last outer position; without it
    /// the OS places the window.
    pub fn open(
        event_loop: &ActiveEventLoop,
        model: &ReaderModel,
        now: Instant,
        position: Option<PhysicalPosition<i32>>,
    ) -> Result<Self, String> {
        let mut attrs = Window::default_attributes()
            .with_title("Smart Voices — Reading")
            .with_inner_size(LogicalSize::new(PANEL_W, PANEL_H))
            .with_min_inner_size(LogicalSize::new(MIN_W, MIN_H))
            .with_resizable(true)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_active(false);
        #[cfg(target_os = "windows")]
        {
            use winit::platform::windows::WindowAttributesExtWindows;
            attrs = attrs.with_skip_taskbar(true);
        }
        #[cfg(target_os = "linux")]
        {
            use winit::platform::x11::{WindowAttributesExtX11, WindowType};
            attrs = attrs.with_x11_window_type(vec![WindowType::Utility]);
        }
        if let Some(position) = position {
            attrs = attrs.with_position(position);
        }
        let window = Rc::new(
            event_loop
                .create_window(attrs)
                .map_err(|err| format!("Couldn't open the reading panel: {err}"))?,
        );
        #[cfg(target_os = "macos")]
        set_device_rgb_color_space(&window);
        let context = Context::new(Rc::clone(&window))
            .map_err(|err| format!("Couldn't open the reading panel: {err}"))?;
        let surface = Surface::new(&context, Rc::clone(&window))
            .map_err(|err| format!("Couldn't open the reading panel: {err}"))?;
        let mut painter =
            Painter::new().map_err(|err| format!("Couldn't open the reading panel: {err}"))?;
        let size = window.inner_size();
        painter.resize(size.width, size.height, window.scale_factor() as f32);
        painter.set_playlist(model.chunks());
        painter.set_highlight(model.current(), model.current_word(now));
        painter.scroll_to_current();
        let theme = match window.theme() {
            Some(WinitTheme::Dark) => Theme::dark(),
            _ => Theme::light(),
        };
        let position = window.outer_position().ok();
        window.request_redraw();
        Ok(Self {
            window,
            context,
            surface,
            painter,
            theme,
            reading_id: model.reading_id(),
            current: model.current(),
            last_key: None,
            user_scrolled: false,
            hovered: false,
            cursor: None,
            position,
            #[cfg(target_os = "macos")]
            previous_app: None,
            #[cfg(target_os = "windows")]
            previous_window: None,
        })
    }

    #[must_use]
    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    /// Catch the painter up with the model. Called after every event batch,
    /// so it only requests a redraw when something visible changed.
    pub fn sync(&mut self, model: &ReaderModel, now: Instant) {
        if model.reading_id() != self.reading_id {
            self.reading_id = model.reading_id();
            self.painter.set_playlist(model.chunks());
            self.user_scrolled = false;
        }
        let current = model.current();
        self.painter.set_highlight(current, model.current_word(now));
        if current != self.current {
            self.current = current;
            if !self.user_scrolled {
                self.painter.scroll_to_current();
            }
        }
        let key = FrameKey::new(model, now, self.painter.scroll_offset());
        if self.last_key.as_ref() != Some(&key) {
            self.last_key = Some(key);
            self.window.request_redraw();
        }
    }

    /// Handle one window event, returning what the app should do.
    pub fn handle(&mut self, event: &WindowEvent, model: &ReaderModel) -> PanelAction {
        match event {
            WindowEvent::RedrawRequested => self.redraw(model),
            WindowEvent::Resized(size) => {
                self.painter
                    .resize(size.width, size.height, self.scale_factor());
                self.window.request_redraw();
                PanelAction::None
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                let size = self.window.inner_size();
                self.painter
                    .resize(size.width, size.height, *scale_factor as f32);
                self.window.request_redraw();
                PanelAction::None
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = Some(*position);
                PanelAction::None
            }
            WindowEvent::CursorEntered { .. } => {
                self.hovered = true;
                #[cfg(target_os = "macos")]
                self.remember_previous_app();
                #[cfg(target_os = "windows")]
                self.remember_previous_window();
                PanelAction::None
            }
            WindowEvent::CursorLeft { .. } => {
                self.hovered = false;
                PanelAction::None
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } => {
                let Some(cursor) = self.cursor else {
                    return PanelAction::None;
                };
                match self.painter.hit(cursor.x as f32, cursor.y as f32) {
                    Hit::Row(index) => PanelAction::Jump(index),
                    Hit::Previous => PanelAction::Previous,
                    Hit::PlayPause => PanelAction::PlayPause,
                    Hit::Next => PanelAction::Next,
                    Hit::None => PanelAction::None,
                }
            }
            WindowEvent::MouseInput { .. } => PanelAction::None,
            WindowEvent::MouseWheel { delta, .. } => {
                self.painter.scroll_by(wheel_px(delta, self.scale_factor()));
                self.user_scrolled = true;
                self.window.request_redraw();
                PanelAction::None
            }
            WindowEvent::ThemeChanged(theme) => {
                self.theme = match theme {
                    WinitTheme::Dark => Theme::dark(),
                    WinitTheme::Light => Theme::light(),
                };
                self.window.request_redraw();
                PanelAction::None
            }
            WindowEvent::Moved(position) => {
                self.position = Some(*position);
                PanelAction::None
            }
            WindowEvent::CloseRequested => PanelAction::Close,
            _ => PanelAction::None,
        }
    }

    #[must_use]
    pub fn is_hovered(&self) -> bool {
        self.hovered
    }

    /// Last outer position, remembered across close/reopen.
    #[must_use]
    pub fn position(&self) -> Option<PhysicalPosition<i32>> {
        self.position
    }

    /// Remember the frontmost app so a panel click can hand focus back.
    #[cfg(target_os = "macos")]
    fn remember_previous_app(&mut self) {
        let here = std::process::id() as i32;
        let workspace = NSWorkspace::sharedWorkspace();
        if let Some(app) = workspace.frontmostApplication()
            && app.processIdentifier() != here
        {
            self.previous_app = Some(app);
        }
    }

    /// Remember the foreground window so a panel click can hand focus back.
    #[cfg(target_os = "windows")]
    fn remember_previous_window(&mut self) {
        use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        // SAFETY: `GetForegroundWindow` takes no arguments and is always valid.
        let foreground = unsafe { GetForegroundWindow() };
        if foreground.is_null() {
            return;
        }
        let own = self
            .window
            .window_handle()
            .ok()
            .and_then(|handle| match handle.as_raw() {
                RawWindowHandle::Win32(win32) => Some(win32.hwnd.get()),
                _ => None,
            });
        if Some(foreground as isize) != own {
            self.previous_window = Some(foreground);
        }
    }

    /// Hand focus back to what was frontmost before the cursor entered.
    /// Called after panel control clicks only, never for close/drag/resize.
    #[cfg(target_os = "macos")]
    pub fn restore_previous_focus(&self) {
        if let Some(app) = &self.previous_app {
            use objc2_app_kit::NSApplicationActivationOptions;
            // The API returns false when activation fails (the app may have
            // quit); there's nothing to report, the click already did its job.
            let _ = app.activateWithOptions(NSApplicationActivationOptions::empty());
        }
    }

    /// Hand focus back to what was foreground before the cursor entered.
    #[cfg(target_os = "windows")]
    pub fn restore_previous_focus(&self) {
        if let Some(previous) = self.previous_window {
            // SAFETY: `previous` was a live foreground window when stored; if
            // it's gone, the call fails harmlessly.
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(previous);
            }
            // Ignore the return: nothing to report if activation fails.
        }
    }

    /// No portable focus restore on this platform.
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    pub fn restore_previous_focus(&self) {}

    fn scale_factor(&self) -> f32 {
        self.window.scale_factor() as f32
    }

    /// Paint one frame straight into the softbuffer and present it.
    fn redraw(&mut self, model: &ReaderModel) -> PanelAction {
        let size = self.window.inner_size();
        let (Some(width), Some(height)) =
            (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return PanelAction::None;
        };
        if let Err(err) = self.surface.resize(width, height) {
            return PanelAction::DrawFailed(format!("Couldn't draw the reading panel: {err}"));
        }
        let mut buffer = match self.surface.buffer_mut() {
            Ok(buffer) => buffer,
            Err(err) => {
                return PanelAction::DrawFailed(format!("Couldn't draw the reading panel: {err}"));
            }
        };
        let label = model.progress_label();
        let frame = Frame {
            label: &label,
            progress: model.progress(),
            can_previous: model.can_previous(),
            can_next: model.can_next(),
            playing: model.state() == PlayerState::Playing,
            can_play_pause: model.is_active(),
            theme: &self.theme,
        };
        self.painter.paint(&frame, &mut buffer);
        match buffer.present() {
            Ok(()) => PanelAction::None,
            Err(err) => PanelAction::DrawFailed(format!("Couldn't draw the reading panel: {err}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use winit::dpi::PhysicalPosition;

    use super::*;
    use crate::player::PlayerEvent;

    #[test]
    fn wheel_deltas_convert_to_pixels() {
        // Line scrolling: 3 lines × 21 px × scale, wheel-up goes negative.
        assert_eq!(
            wheel_px(&MouseScrollDelta::LineDelta(0.0, 1.0), 2.0),
            -126.0
        );
        assert_eq!(wheel_px(&MouseScrollDelta::LineDelta(0.0, -1.0), 1.0), 63.0);
        assert_eq!(
            wheel_px(&MouseScrollDelta::LineDelta(5.0, 0.5), 1.5),
            -0.5 * 3.0 * 21.0 * 1.5
        );
        // Pixel scrolling passes the y delta through, negated.
        assert_eq!(
            wheel_px(
                &MouseScrollDelta::PixelDelta(PhysicalPosition::new(10.0, 40.0)),
                2.0
            ),
            -40.0
        );
    }

    fn playing_model(start: Instant) -> ReaderModel {
        let mut model = ReaderModel::new();
        model.apply(
            &PlayerEvent::Playlist(
                ["aaa bbb ccc", "ddd eee fff"]
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
            ),
            start,
        );
        model.apply(&PlayerEvent::State(PlayerState::Playing), start);
        model.apply(
            &PlayerEvent::Chunk {
                index: 0,
                count: 2,
                text: "aaa bbb ccc".to_string(),
                duration: Duration::from_secs(12),
            },
            start,
        );
        model
    }

    #[test]
    fn same_model_and_time_keep_the_frame_key() {
        let start = Instant::now();
        let model = playing_model(start);
        // The same model and time compare equal: `sync` requests no redraw.
        assert_eq!(
            FrameKey::new(&model, start, 0.0),
            FrameKey::new(&model, start, 0.0)
        );
        // Still equal a moment later, while the word hasn't changed…
        assert_eq!(
            FrameKey::new(&model, start, 0.0),
            FrameKey::new(&model, start + Duration::from_millis(100), 0.0)
        );
        // …but a new word, label, or scroll offset changes the key.
        assert_ne!(
            FrameKey::new(&model, start, 0.0),
            FrameKey::new(&model, start + Duration::from_secs(5), 0.0)
        );
        assert_ne!(
            FrameKey::new(&model, start, 0.0),
            FrameKey::new(&model, start, 21.0)
        );
        let mut advanced = playing_model(start);
        advanced.apply(
            &PlayerEvent::Chunk {
                index: 1,
                count: 2,
                text: "ddd eee fff".to_string(),
                duration: Duration::from_secs(12),
            },
            start,
        );
        assert_ne!(
            FrameKey::new(&model, start, 0.0),
            FrameKey::new(&advanced, start, 0.0)
        );
    }
}
