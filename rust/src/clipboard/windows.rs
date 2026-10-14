//! Windows clipboard watcher: event-driven, no timer. A message-only window
//! (`HWND_MESSAGE`) of a registered listener class with `AddClipboardFormatListener`
//! receives `WM_CLIPBOARDUPDATE` in its own `wndproc`; the watcher thread runs a
//! plain `GetMessageW` dispatch loop. Contents are never read by the watcher,
//! only by [`read_text`].
//!
//! Note: compile-unknown on macOS/Linux (needs the Windows target); kept
//! small and written against `windows-sys` 0.61 signatures.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use windows_sys::Win32::Foundation::{GetLastError, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, GetClipboardData, IsClipboardFormatAvailable,
    OpenClipboard, RegisterClipboardFormatW, RemoveClipboardFormatListener,
};
use windows_sys::Win32::System::Diagnostics::Debug::{
    FORMAT_MESSAGE_FROM_SYSTEM, FORMAT_MESSAGE_IGNORE_INSERTS, FormatMessageW,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows_sys::Win32::System::Ole::CF_UNICODETEXT;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA, GetMessageW,
    GetWindowLongPtrW, HWND_MESSAGE, MSG, PostMessageW, PostQuitMessage, RegisterClassW,
    SetWindowLongPtrW, TranslateMessage, WM_CLIPBOARDUPDATE, WM_CLOSE, WM_NCDESTROY, WNDCLASSW,
};

use super::{
    ClipboardError, ClipboardEvent, WINDOWS_HISTORY_OPT_OUT, WINDOWS_SECRET_FORMATS,
    windows_should_notify,
};

/// `CF_UNICODETEXT` as the `u32` the clipboard APIs take
/// (`CLIPBOARD_FORMAT` itself is a `u16`).
const CF_TEXT: u32 = CF_UNICODETEXT as u32;

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
        // Wake the `GetMessageW` loop; `wndproc` quits on `WM_CLOSE`.
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

/// State shared with `wndproc`, owned by the window: stored in
/// `GWLP_USERDATA` after creation, reclaimed on `WM_NCDESTROY`.
struct ListenerState {
    events: mpsc::Sender<ClipboardEvent>,
    enabled: Arc<AtomicBool>,
    formats: FormatIds,
}

/// Window procedure for the listener window: announce new text on
/// `WM_CLIPBOARDUPDATE` (formats only, never contents), quit on `WM_CLOSE`.
unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: `hwnd` is our listener window; `GWLP_USERDATA` holds a
    // `Box<ListenerState>` from creation until `WM_NCDESTROY`.
    let state = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const ListenerState };
    match msg {
        WM_NCDESTROY => {
            if !state.is_null() {
                unsafe {
                    drop(Box::from_raw(state as *mut ListenerState));
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                }
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_CLIPBOARDUPDATE => {
            if !state.is_null() {
                let state = unsafe { &*state };
                if state.enabled.load(Ordering::Relaxed) && should_announce(&state.formats) {
                    let _ = state
                        .events
                        .send(ClipboardEvent::Changed { source_app: None });
                }
            }
            0
        }
        WM_CLOSE => {
            // Stop request from `Drop`: quit the message loop. The window is
            // destroyed by `run_loop` after the loop exits.
            unsafe {
                PostQuitMessage(0);
            }
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn run_loop(
    events: mpsc::Sender<ClipboardEvent>,
    enabled: Arc<AtomicBool>,
    ready: mpsc::Sender<Result<HWND, ClipboardError>>,
) {
    let state = Box::new(ListenerState {
        events: events.clone(),
        enabled,
        formats: FormatIds {
            exclude: register_format(WINDOWS_SECRET_FORMATS[0]),
            ignore: register_format(WINDOWS_SECRET_FORMATS[1]),
            history: register_format(WINDOWS_HISTORY_OPT_OUT),
        },
    });
    let hwnd = match create_listener_window(state) {
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
    let _ = ready.send(Ok(hwnd));
    message_loop(&events);
    unsafe {
        RemoveClipboardFormatListener(hwnd);
        DestroyWindow(hwnd);
    }
}

fn message_loop(events: &mpsc::Sender<ClipboardEvent>) {
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
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Register the listener window class and create its message-only window.
/// Ownership of `state` moves to the window (`GWLP_USERDATA`); it is freed on
/// `WM_NCDESTROY`, or here if creation fails.
fn create_listener_window(state: Box<ListenerState>) -> Result<HWND, ClipboardError> {
    let class_name = wide("SmartVoicesClipboardListener");
    // SAFETY: null module name queries the current module; the returned
    // handle is only passed to `RegisterClassW`/`CreateWindowExW`.
    let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
    if instance.is_null() {
        return Err(last_error(
            "Couldn't get the module handle for the clipboard listener",
        ));
    }
    let class = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        hInstance: instance,
        lpszClassName: class_name.as_ptr(),
        ..Default::default()
    };
    // SAFETY: `class` is fully initialized and `lpszClassName` is alive for
    // the call; the system copies the name.
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(last_error(
            "Couldn't register the clipboard listener window class",
        ));
    }
    // SAFETY: all pointers are valid (class name is registered) or null;
    // `HWND_MESSAGE` makes a message-only window.
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
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
    // SAFETY: hands `state` to the window; `wndproc` reclaims it exactly once
    // on `WM_NCDESTROY`. No `WM_CLIPBOARDUPDATE` can arrive before the
    // listener is added after this returns, and `wndproc` null-checks anyway.
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);
    }
    Ok(hwnd)
}

/// Decide whether a clipboard update announces new text: require
/// `CF_UNICODETEXT`, skip password-manager markers. Reads at most one DWORD
/// (the history opt-out value), never text contents.
fn should_announce(formats: &FormatIds) -> bool {
    let available = |format: u32| format != 0 && unsafe { IsClipboardFormatAvailable(format) } != 0;
    // SAFETY: pure availability queries, no clipboard ownership needed.
    let has_text = unsafe { IsClipboardFormatAvailable(CF_TEXT) } != 0;
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
    if unsafe { IsClipboardFormatAvailable(CF_TEXT) } == 0 {
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
        let handle = GetClipboardData(CF_TEXT);
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
    let code = unsafe { GetLastError() };
    match os_error_text(code) {
        Some(text) => ClipboardError::Backend(format!("{step}: {text} (code {code}).")),
        None => ClipboardError::Backend(format!("{step}: Windows error {code}.")),
    }
}

/// Render a Win32 error code as text (`FormatMessageW`); `None` when the
/// system has no message for the code.
fn os_error_text(code: u32) -> Option<String> {
    let mut buffer = [0u16; 512];
    // SAFETY: the buffer is valid for its length in UTF-16 units; system
    // message table, no inserts, neutral language.
    let len = unsafe {
        FormatMessageW(
            FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS,
            std::ptr::null(),
            code,
            0,
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            std::ptr::null(),
        )
    };
    if len == 0 {
        return None;
    }
    let text = String::from_utf16_lossy(&buffer[..len as usize]);
    let text = text.trim().to_string();
    if text.is_empty() { None } else { Some(text) }
}
