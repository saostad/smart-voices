//! macOS clipboard watcher: `NSPasteboard.changeCount` checked every 500 ms
//! (AppKit offers no change notification; this is the one allowed timer).
//! Contents are never read by the watcher, only by [`read_text`].

use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use objc2::rc::Retained;
use objc2::rc::autoreleasepool;
use objc2::{class, msg_send};
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString, NSWorkspace};
use objc2_foundation::NSInteger;

use super::{ClipboardError, ClipboardEvent, macos_should_notify};

/// Poll cadence (same as Swift's 0.5 s timer).
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Error when the system returns no pasteboard (seen in sandboxes without a
/// pasteboard service).
const NO_PASTEBOARD: &str = "Couldn't access the clipboard (the system returned no pasteboard).";

/// The general pasteboard, or `None` when the system returns NULL.
/// The generated `NSPasteboard::generalPasteboard()` binding aborts the
/// process on NULL, so this calls the selector directly instead.
fn general_pasteboard() -> Option<Retained<NSPasteboard>> {
    // SAFETY: `+generalPasteboard` takes no arguments; a NULL return becomes
    // `None` instead of aborting.
    unsafe { msg_send![class!(NSPasteboard), generalPasteboard] }
}

/// Commands for the watcher thread; `recv_timeout(POLL_INTERVAL)` doubles as
/// the poll timer, so there is no separate timer.
enum Control {
    Stop,
    SetEnabled(bool),
}

pub(super) struct Watcher {
    control: mpsc::Sender<Control>,
    thread: Option<JoinHandle<()>>,
}

impl Watcher {
    pub(super) fn set_enabled(&self, enabled: bool) {
        // The thread is gone only after `Drop`; ignore a failed send.
        let _ = self.control.send(Control::SetEnabled(enabled));
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        let _ = self.control.send(Control::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub(super) fn watch(events: mpsc::Sender<ClipboardEvent>) -> Result<Watcher, ClipboardError> {
    let (control_tx, control_rx) = mpsc::channel();
    let thread = thread::Builder::new()
        .name("clipboard-watch".to_string())
        .spawn(move || run_loop(events, control_rx))
        .map_err(|err| {
            ClipboardError::Backend(format!(
                "Couldn't start the clipboard watcher thread: {err}"
            ))
        })?;
    Ok(Watcher {
        control: control_tx,
        thread: Some(thread),
    })
}

fn run_loop(events: mpsc::Sender<ClipboardEvent>, control: mpsc::Receiver<Control>) {
    let mut enabled = true;
    let Some(mut last) = change_count() else {
        let _ = events.send(ClipboardEvent::Error(NO_PASTEBOARD.to_string()));
        return;
    };
    loop {
        match control.recv_timeout(POLL_INTERVAL) {
            Ok(Control::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Ok(Control::SetEnabled(next)) => {
                enabled = next;
                if enabled {
                    // Don't report copies made while paused (Swift parity:
                    // `isEnabled` resets `lastChangeCount` on enable).
                    // A NULL pasteboard here is reported by the next poll.
                    if let Some(current) = change_count() {
                        last = current;
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if !enabled {
                    continue;
                }
                match poll(&mut last) {
                    Ok(Some(event)) => {
                        if events.send(event).is_err() {
                            break;
                        }
                    }
                    Ok(None) => {}
                    Err(err) => {
                        let _ = events.send(ClipboardEvent::Error(err.to_string()));
                        break;
                    }
                }
            }
        }
    }
}

/// Check for a reportable change, updating `last`. All pasteboard reads run in
/// an autoreleasepool, as required off the main thread.
fn poll(last: &mut NSInteger) -> Result<Option<ClipboardEvent>, ClipboardError> {
    autoreleasepool(|_| {
        let Some(pasteboard) = general_pasteboard() else {
            return Err(ClipboardError::Backend(NO_PASTEBOARD.to_string()));
        };
        let current = pasteboard.changeCount();
        if current == *last {
            return Ok(None);
        }
        *last = current;
        let types: Vec<String> = pasteboard
            .types()
            .map(|array| array.to_vec().iter().map(|text| text.to_string()).collect())
            .unwrap_or_default();
        let refs: Vec<&str> = types.iter().map(String::as_str).collect();
        if !macos_should_notify(&refs) {
            return Ok(None);
        }
        Ok(Some(ClipboardEvent::Changed {
            source_app: frontmost_app_name(),
        }))
    })
}

fn change_count() -> Option<NSInteger> {
    autoreleasepool(|_| general_pasteboard().map(|pasteboard| pasteboard.changeCount()))
}

fn frontmost_app_name() -> Option<String> {
    // Called inside `poll`'s autoreleasepool.
    let workspace = NSWorkspace::sharedWorkspace();
    let app = workspace.frontmostApplication()?;
    let name = app.localizedName()?;
    Some(name.to_string())
}

pub(super) fn read_text() -> Result<Option<String>, ClipboardError> {
    autoreleasepool(|_| {
        let Some(pasteboard) = general_pasteboard() else {
            return Err(ClipboardError::Backend(NO_PASTEBOARD.to_string()));
        };
        // SAFETY: copying the global string-type constant out of its `extern`
        // static; the constant is always valid.
        let string_type = unsafe { NSPasteboardTypeString };
        Ok(pasteboard
            .stringForType(string_type)
            .map(|text| text.to_string()))
    })
}
