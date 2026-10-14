//! Linux clipboard watcher: X11 via `x11rb` (works on X11 sessions and
//! through XWayland). Event-driven, no timer: the watcher thread owns one
//! connection plus a 1×1 unmapped window, subscribes to XFixes
//! `SET_SELECTION_OWNER` on `CLIPBOARD`, and blocks in `wait_for_event`.
//! Contents are never read by the watcher, only by [`read_text`] (which uses
//! its own short-lived connection, since a connection can't be shared with a
//! thread blocked in `wait_for_event`).
//!
//! Note: compile-unknown on macOS/Windows (needs the Linux target and an X
//! server at runtime); kept small and written against `x11rb` 0.14 signatures.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xfixes::{self, SelectionEvent, SelectionEventMask};
use x11rb::protocol::xproto::{
    self, Atom, ClientMessageData, ClientMessageEvent, CreateWindowAux, EventMask, Window,
    WindowClass,
};
use x11rb::rust_connection::RustConnection;

use super::{ClipboardError, ClipboardEvent, LINUX_PASSWORD_HINT, linux_should_notify};

/// How long a selection conversion may take before it counts as failed.
const CONVERT_TIMEOUT: Duration = Duration::from_secs(1);
/// Event-queue poll step while waiting for a conversion (bounded wait, not a
/// poll loop: at most 1 s, only while answering a change or an explicit read).
const CONVERT_POLL_STEP: Duration = Duration::from_millis(5);

pub(super) struct Watcher {
    window: Window,
    stop_atom: Atom,
    /// Second connection, used only to wake the watcher thread out of
    /// `wait_for_event` from `Drop`.
    wake: RustConnection,
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
        // Wake the thread: it exits on this message (see `event_loop`).
        let stop = ClientMessageEvent::new(
            32,
            self.window,
            self.stop_atom,
            ClientMessageData::from([0u32; 5]),
        );
        let _ = xproto::send_event(
            &self.wake,
            false,
            self.window,
            EventMask::STRUCTURE_NOTIFY,
            stop,
        );
        let _ = self.wake.flush();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub(super) fn watch(events: mpsc::Sender<ClipboardEvent>) -> Result<Watcher, ClipboardError> {
    if std::env::var_os("DISPLAY").is_none() {
        return Err(ClipboardError::Unsupported(
            "Watching the clipboard isn't available on this desktop (Wayland without X11). Use the Read Clipboard shortcut instead."
                .to_string(),
        ));
    }
    // The wake connection never crosses threads; connecting it here fails
    // fast when the X server is unreachable.
    let (wake, _) =
        x11rb::connect(None).map_err(|err| x11("Couldn't connect to the X server", err))?;
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
        Ok(Ok(stop)) => Ok(Watcher {
            window: stop.window,
            stop_atom: stop.atom,
            wake,
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

/// What `Drop` needs to stop the thread.
struct StopInfo {
    window: Window,
    atom: Atom,
}

/// Interned atoms used by the watcher thread and [`read_text`].
struct Atoms {
    clipboard: Atom,
    utf8_string: Atom,
    targets: Atom,
    incr: Atom,
    hint: Atom,
    property: Atom,
    stop: Atom,
}

fn run_loop(
    events: mpsc::Sender<ClipboardEvent>,
    enabled: Arc<AtomicBool>,
    ready: mpsc::Sender<Result<StopInfo, ClipboardError>>,
) {
    let (conn, window, atoms) = match setup_watcher() {
        Ok(setup) => setup,
        Err(err) => {
            let _ = ready.send(Err(err));
            return;
        }
    };
    let _ = ready.send(Ok(StopInfo {
        window,
        atom: atoms.stop,
    }));
    event_loop(&conn, window, &atoms, &events, &enabled);
    let _ = xproto::destroy_window(&conn, window);
}

fn setup_watcher() -> Result<(RustConnection, Window, Atoms), ClipboardError> {
    let (conn, screen) =
        x11rb::connect(None).map_err(|err| x11("Couldn't connect to the X server", err))?;
    let window = create_window(&conn, screen)?;
    let atoms = intern_atoms(&conn)?;
    xfixes::query_version(&conn, 5, 0)
        .map_err(|err| x11("Couldn't negotiate the XFixes version", err))?
        .reply()
        .map_err(|err| x11("Couldn't negotiate the XFixes version", err))?;
    xfixes::select_selection_input(
        &conn,
        window,
        atoms.clipboard,
        SelectionEventMask::SET_SELECTION_OWNER,
    )
    .map_err(|err| x11("Couldn't subscribe to clipboard changes", err))?
    .check()
    .map_err(|err| x11("Couldn't subscribe to clipboard changes", err))?;
    conn.flush()
        .map_err(|err| x11("Couldn't subscribe to clipboard changes", err))?;
    Ok((conn, window, atoms))
}

fn event_loop(
    conn: &RustConnection,
    window: Window,
    atoms: &Atoms,
    events: &mpsc::Sender<ClipboardEvent>,
    enabled: &AtomicBool,
) {
    // Events observed while waiting for a conversion reply, replayed first.
    let mut stashed: VecDeque<Event> = VecDeque::new();
    loop {
        let event = if let Some(event) = stashed.pop_front() {
            event
        } else {
            match conn.wait_for_event() {
                Ok(event) => event,
                Err(err) => {
                    let _ = events.send(ClipboardEvent::Error(
                        x11("Lost the connection to the X server", err).to_string(),
                    ));
                    break;
                }
            }
        };
        match event {
            Event::XfixesSelectionNotify(notify)
                if notify.selection == atoms.clipboard
                    && notify.subtype == SelectionEvent::SET_SELECTION_OWNER =>
            {
                if notify.owner == x11rb::NONE {
                    continue; // Clipboard cleared, not set.
                }
                if !enabled.load(Ordering::Relaxed) {
                    continue;
                }
                // Never read contents here; only check the password hint.
                match is_secret(conn, window, atoms, &mut stashed) {
                    Ok(true) => {}
                    Ok(false) => {
                        if events
                            .send(ClipboardEvent::Changed { source_app: None })
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(err) => {
                        let _ = events.send(ClipboardEvent::Error(err.to_string()));
                        break;
                    }
                }
            }
            Event::ClientMessage(message)
                if message.window == window && message.type_ == atoms.stop =>
            {
                break;
            }
            _ => {}
        }
    }
}

/// Check whether the new selection owner marks the copy as a password.
/// Conversion failures and timeouts mean "can't confirm": not secret.
fn is_secret(
    conn: &RustConnection,
    window: Window,
    atoms: &Atoms,
    stashed: &mut VecDeque<Event>,
) -> Result<bool, ClipboardError> {
    let targets = match request_selection(conn, window, atoms, atoms.targets, stashed)? {
        Selection::Converted(bytes) => bytes,
        Selection::Failed | Selection::Incr => return Ok(false),
    };
    // Compare raw bytes: correct regardless of the server's byte order.
    let hint = atoms.hint.to_ne_bytes();
    if !targets.chunks_exact(4).any(|chunk| chunk == hint) {
        return Ok(false);
    }
    let value = match request_selection(conn, window, atoms, atoms.hint, stashed)? {
        Selection::Converted(bytes) => bytes,
        Selection::Failed | Selection::Incr => return Ok(false),
    };
    let text = std::str::from_utf8(&value)
        .unwrap_or_default()
        .trim_end_matches('\0');
    Ok(!linux_should_notify(&[LINUX_PASSWORD_HINT], Some(text)))
}

/// Outcome of one `convert_selection` round-trip.
enum Selection {
    /// The owner wrote the converted value into our property.
    Converted(Vec<u8>),
    /// The owner refused or never answered (property `None` / timeout).
    Failed,
    /// INCR transfer: the value is too large for one property.
    Incr,
}

/// Convert `CLIPBOARD` to `target`, waiting up to 1 s for the owner's reply.
/// Events for anything else are pushed to `stashed` for the caller.
fn request_selection(
    conn: &RustConnection,
    window: Window,
    atoms: &Atoms,
    target: Atom,
    stashed: &mut VecDeque<Event>,
) -> Result<Selection, ClipboardError> {
    xproto::convert_selection(
        conn,
        window,
        atoms.clipboard,
        target,
        atoms.property,
        x11rb::CURRENT_TIME,
    )
    .map_err(|err| x11("Couldn't ask the clipboard owner for its text", err))?;
    conn.flush()
        .map_err(|err| x11("Couldn't ask the clipboard owner for its text", err))?;
    let deadline = Instant::now() + CONVERT_TIMEOUT;
    loop {
        match conn
            .poll_for_event()
            .map_err(|err| x11("Couldn't read the X11 event queue", err))?
        {
            Some(Event::SelectionNotify(notify))
                if notify.requestor == window
                    && notify.selection == atoms.clipboard
                    && notify.target == target =>
            {
                if notify.property == x11rb::NONE {
                    return Ok(Selection::Failed);
                }
                let reply = xproto::get_property(
                    conn,
                    true,
                    window,
                    notify.property,
                    x11rb::NONE,
                    0,
                    u32::MAX / 4,
                )
                .map_err(|err| x11("Couldn't read the clipboard reply", err))?
                .reply()
                .map_err(|err| x11("Couldn't read the clipboard reply", err))?;
                if reply.type_ == atoms.incr {
                    return Ok(Selection::Incr);
                }
                return Ok(Selection::Converted(reply.value));
            }
            Some(event) => stashed.push_back(event),
            None => {
                if Instant::now() >= deadline {
                    return Ok(Selection::Failed);
                }
                std::thread::sleep(CONVERT_POLL_STEP);
            }
        }
    }
}

pub(super) fn read_text() -> Result<Option<String>, ClipboardError> {
    // Its own connection: the watcher thread (if any) blocks in
    // `wait_for_event` on its connection and can't share it.
    let (conn, screen) =
        x11rb::connect(None).map_err(|err| x11("Couldn't connect to the X server", err))?;
    let window = create_window(&conn, screen)?;
    let atoms = intern_atoms(&conn)?;
    // Private connection: only our own reply arrives; anything else is dropped.
    let mut stashed = VecDeque::new();
    let result = match request_selection(&conn, window, &atoms, atoms.utf8_string, &mut stashed)? {
        Selection::Converted(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            Ok(Some(text.trim_end_matches('\0').to_string()))
        }
        Selection::Failed => Ok(None),
        Selection::Incr => Err(ClipboardError::Backend(
            "The copied text is too large to read on this desktop.".to_string(),
        )),
    };
    let _ = xproto::destroy_window(&conn, window);
    result
}

/// Create our 1×1 unmapped window. It selects `STRUCTURE_NOTIFY` so the stop
/// message (sent with that mask) is delivered back to our connection.
fn create_window(conn: &RustConnection, screen: usize) -> Result<Window, ClipboardError> {
    let window = conn
        .generate_id()
        .map_err(|err| x11("Couldn't allocate an X11 window", err))?;
    let root = conn.setup().roots[screen].root;
    xproto::create_window(
        conn,
        x11rb::COPY_DEPTH_FROM_PARENT,
        window,
        root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_OUTPUT,
        x11rb::COPY_FROM_PARENT,
        &CreateWindowAux::new().event_mask(EventMask::STRUCTURE_NOTIFY),
    )
    .map_err(|err| x11("Couldn't create the clipboard window", err))?
    .check()
    .map_err(|err| x11("Couldn't create the clipboard window", err))?;
    Ok(window)
}

fn intern_atoms(conn: &RustConnection) -> Result<Atoms, ClipboardError> {
    let intern = |name: &[u8]| {
        xproto::intern_atom(conn, false, name)
            .map_err(|err| x11("Couldn't look up X11 atoms", err))?
            .reply()
            .map(|reply| reply.atom)
            .map_err(|err| x11("Couldn't look up X11 atoms", err))
    };
    Ok(Atoms {
        clipboard: intern(b"CLIPBOARD")?,
        utf8_string: intern(b"UTF8_STRING")?,
        targets: intern(b"TARGETS")?,
        incr: intern(b"INCR")?,
        hint: intern(LINUX_PASSWORD_HINT.as_bytes())?,
        property: intern(b"SMART_VOICES_SELECTION")?,
        stop: intern(b"SMART_VOICES_STOP")?,
    })
}

fn x11(step: &str, err: impl std::fmt::Display) -> ClipboardError {
    ClipboardError::Backend(format!("{step}: {err}"))
}
