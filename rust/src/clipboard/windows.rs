//! Windows clipboard watcher: event-driven, no timer. A message-only window
//! (`HWND_MESSAGE`) with `AddClipboardFormatListener` receives
//! `WM_CLIPBOARDUPDATE`; the watcher thread's `GetMessageW` loop observes it
//! directly. Contents are never read by the watcher, only by [`read_text`].
//!
//! Note: compile-unknown on macOS/Linux (needs the Windows target); kept
//! small and written against `windows-sys` 0.61 signatures.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use windows_sys::Win32::Foundation::{GetLastError, HWND};
use windows_sys::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, GetClipboardData, IsClipboardFormatAvailable,
    OpenClipboard, RegisterClipboardFormatW, RemoveClipboardFormatListener,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, DispatchMessageW, GetMessageW, HWND_MESSAGE, MSG, PostMessageW,
    PostQuitMessage, TranslateMessage, WM_CLIPBOARDUPDATE, WM_CLOSE,
};

use super::{
    ClipboardError, ClipboardEvent, WINDOWS_HISTORY_OPT_OUT, WINDOWS_SECRET_FORMATS,
    windows_should_notify,
};

/// `CF_UNICODETEXT` (13). It lives in `Win32_System_Ole`, which this crate
/// deliberately doesn't pull in, so the SDK value is spelled out here.
const CF_UNICODETEXT: u32 = 13;

/// Open attempts for [`read_text`]: another app may hold the clipboard open.
const OPEN_RETRIES: u32 = 5;
/// Delay between open attempts (bounded wait, not a poll loop).
const OPEN_RETRY_DELAY: Duration = Duration::from_millis(20);

pub(super) struct Watcher {
    hwnd: HWND,
    enabled: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Watcher {
    pub(super) fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        // Wake the `GetMessageW` loop; it destroys the window and quits.
        unsafe {
            PostMessageW(self.hwnd, WM_CLOSE, 0, 0);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub(super) fn watch(events: mpsc::Sender<ClipboardEvent>) -> Result<Watcher, ClipboardError> {
    let enabled = Arc::new(AtomicBool::new(true));
    let (ready_tx, ready_rx) = mpsc::channel();
    let thread_enabled = Arc::clone(&enabled);
    let thread = thread::Builder::new()
        .name("clipboard-watch".to_string())
        .spawn(move || run_loop(events, thread_enabled, ready_tx))
        .map_err(|err| {
            ClipboardError::Backend(format!(
                "Couldn't start the clipboard watcher thread: {err}"
            ))
        })?;
    match ready_rx.recv() {
        Ok(Ok(hwnd)) => Ok(Watcher {
            hwnd,
            enabled,
            thread: Some(thread),
        }),
        Ok(Err(err)) => {
            let _ = thread.join();
            Err(err)
        }
        Err(_) => {
            let _ = thread.join();
            Err(ClipboardError::Backend(
                "The clipboard watcher thread stopped before it was ready.".to_string(),
            ))
        }
    }
}

/// Custom-format ids, cached once (0 = registration failed; that marker is
/// then ignored because format 0 is never available).
struct FormatIds {
    exclude: u32,
    ignore: u32,
    history: u32,
}

fn run_loop(
    events: mpsc::Sender<ClipboardEvent>,
    enabled: Arc<AtomicBool>,
    ready: mpsc::Sender<Result<HWND, ClipboardError>>,
) {
    let hwnd = match create_listener_window() {
        Ok(hwnd) => hwnd,
        Err(err) => {
            let _ = ready.send(Err(err));
            return;
        }
    };
    if unsafe { AddClipboardFormatListener(hwnd) } == 0 {
        let err = last_error("Couldn't listen for clipboard updates");
        unsafe {
            DestroyWindow(hwnd);
        }
        let _ = ready.send(Err(err));
        return;
    }
    let formats = FormatIds {
        exclude: register_format(WINDOWS_SECRET_FORMATS[0]),
        ignore: register_format(WINDOWS_SECRET_FORMATS[1]),
        history: register_format(WINDOWS_HISTORY_OPT_OUT),
    };
    let _ = ready.send(Ok(hwnd));
    message_loop(hwnd, &formats, &events, &enabled);
    unsafe {
        RemoveClipboardFormatListener(hwnd);
        DestroyWindow(hwnd);
    }
}

fn message_loop(
    hwnd: HWND,
    formats: &FormatIds,
    events: &mpsc::Sender<ClipboardEvent>,
    enabled: &AtomicBool,
) {
    // SAFETY: `GetMessageW` writes a full `MSG` for this thread's windows;
    // `msg` starts zeroed and is only read after a positive return.
    let mut msg = MSG::default();
    loop {
        let status = unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) };
        if status == 0 {
            break; // WM_QUIT.
        }
        if status == -1 {
            let _ = events.send(ClipboardEvent::Error(
                last_error("Couldn't read window messages").to_string(),
            ));
            break;
        }
        if msg.message == WM_CLOSE && msg.hwnd == hwnd {
            // Stop request from `Drop`: destroy the window and quit. The
            // message is consumed here, not dispatched.
            unsafe {
                PostQuitMessage(0);
            }
            continue;
        }
        if msg.message == WM_CLIPBOARDUPDATE && enabled.load(Ordering::Relaxed) {
            // The watcher checks formats only, never contents.
            if should_announce(formats)
                && events
                    .send(ClipboardEvent::Changed { source_app: None })
                    .is_err()
            {
                break;
            }
        }
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Create the message-only listener window. The system class `STATIC` is
/// pre-registered for every process; using it avoids `RegisterClass`, whose
/// `WNDCLASS` structs would pull in the `Win32_Graphics_Gdi` feature.
/// `WM_CLIPBOARDUPDATE` is observed directly in the message loop, so no custom
/// window procedure is needed (unhandled messages go to `DefWindowProcW`).
fn create_listener_window() -> Result<HWND, ClipboardError> {
    let class = wide("STATIC");
    // SAFETY: null module name queries the current module; the returned
    // handle is only passed to `CreateWindowExW`.
    let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
    if instance.is_null() {
        return Err(last_error(
            "Couldn't get the module handle for the clipboard listener",
        ));
    }
    // SAFETY: all pointers are valid (class name is NUL-terminated) or null;
    // `HWND_MESSAGE` makes a message-only window.
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            std::ptr::null(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        )
    };
    if hwnd.is_null() {
        return Err(last_error("Couldn't create the clipboard listener window"));
    }
    Ok(hwnd)
}

/// Decide whether a clipboard update announces new text: require
/// `CF_UNICODETEXT`, skip password-manager markers. Reads at most one DWORD
/// (the history opt-out value), never text contents.
fn should_announce(formats: &FormatIds) -> bool {
    let available = |format: u32| format != 0 && unsafe { IsClipboardFormatAvailable(format) } != 0;
    // SAFETY: pure availability queries, no clipboard ownership needed.
    let has_text = unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT) } != 0;
    let mut present = Vec::with_capacity(3);
    if available(formats.exclude) {
        present.push(WINDOWS_SECRET_FORMATS[0]);
    }
    if available(formats.ignore) {
        present.push(WINDOWS_SECRET_FORMATS[1]);
    }
    let history_value = if available(formats.history) {
        present.push(WINDOWS_HISTORY_OPT_OUT);
        clipboard_dword(formats.history)
    } else {
        None
    };
    windows_should_notify(has_text, &present, history_value)
}

/// Read the first DWORD of a custom clipboard format (single attempt; the
/// watcher must not stall its message loop). `None` when unavailable.
fn clipboard_dword(format: u32) -> Option<u32> {
    // SAFETY: the clipboard is opened, the locked bytes are read within
    // `GlobalSize`, then unlocked and closed on every path.
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return None;
        }
        let value = clipboard_bytes(format).and_then(|bytes| {
            bytes
                .get(0..4)
                .map(|word| u32::from_ne_bytes([word[0], word[1], word[2], word[3]]))
        });
        CloseClipboard();
        value
    }
}

/// Copy the raw bytes of a custom clipboard format. The caller must hold the
/// clipboard open.
unsafe fn clipboard_bytes(format: u32) -> Option<Vec<u8>> {
    // SAFETY: caller holds the clipboard open; the handle's `GlobalSize`
    // bytes are valid between lock and unlock.
    unsafe {
        let handle = GetClipboardData(format);
        if handle.is_null() {
            return None;
        }
        let locked = GlobalLock(handle);
        if locked.is_null() {
            return None;
        }
        let len = GlobalSize(handle);
        let bytes = std::slice::from_raw_parts(locked as *const u8, len).to_vec();
        GlobalUnlock(handle);
        Some(bytes)
    }
}

pub(super) fn read_text() -> Result<Option<String>, ClipboardError> {
    // SAFETY: pure availability query, no clipboard ownership needed.
    if unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT) } == 0 {
        return Ok(None);
    }
    open_clipboard()?;
    // Every path below closes the clipboard exactly once.
    let result = read_unicode_text();
    unsafe {
        CloseClipboard();
    }
    result
}

/// Open the clipboard, retrying briefly: another app may hold it open.
fn open_clipboard() -> Result<(), ClipboardError> {
    for attempt in 0..OPEN_RETRIES {
        // SAFETY: null owner is allowed; success/failure is checked.
        if unsafe { OpenClipboard(std::ptr::null_mut()) } != 0 {
            return Ok(());
        }
        if attempt + 1 < OPEN_RETRIES {
            std::thread::sleep(OPEN_RETRY_DELAY);
        }
    }
    Err(last_error("Couldn't open the clipboard"))
}

/// Read `CF_UNICODETEXT`. The caller must hold the clipboard open.
fn read_unicode_text() -> Result<Option<String>, ClipboardError> {
    // SAFETY: the caller holds the clipboard open (see `read_text`); the
    // handle's bytes are valid between lock and unlock.
    unsafe {
        let handle = GetClipboardData(CF_UNICODETEXT);
        if handle.is_null() {
            return Err(last_error("Couldn't read text from the clipboard"));
        }
        let locked = GlobalLock(handle);
        if locked.is_null() {
            return Err(last_error("Couldn't lock the clipboard text"));
        }
        let units = GlobalSize(handle) / 2;
        let text = std::slice::from_raw_parts(locked as *const u16, units);
        let end = text.iter().position(|unit| *unit == 0).unwrap_or(units);
        let text = String::from_utf16_lossy(&text[..end]);
        GlobalUnlock(handle);
        Ok(Some(text))
    }
}

fn register_format(name: &str) -> u32 {
    let wide = wide(name);
    // SAFETY: the name is NUL-terminated and alive for the call.
    unsafe { RegisterClipboardFormatW(wide.as_ptr()) }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn last_error(step: &str) -> ClipboardError {
    // SAFETY: `GetLastError` takes no arguments and is always valid to call.
    ClipboardError::Backend(format!("{step}: Windows error {}.", unsafe {
        GetLastError()
    }))
}
