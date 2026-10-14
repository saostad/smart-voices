//! Global shortcuts on Wayland via the XDG GlobalShortcuts portal.
//!
//! `global-hotkey` grabs keys through X11, which can't see key presses from
//! native Wayland apps, so on Wayland sessions (`WAYLAND_DISPLAY` set) the
//! app binds its shortcuts through the desktop portal instead (GNOME 48+,
//! KDE 6). The desktop may show its own confirmation dialog the first time;
//! ids the desktop already granted are left alone so the dialog doesn't
//! reappear on every launch.
//!
//! Linux-only; started on a dedicated thread from `register_shortcuts` in
//! `app.rs`, which also keeps the `global-hotkey` path for X11 sessions.

use std::collections::HashSet;
use std::future::poll_fn;

use ashpd::desktop::CreateSessionOptions;
use ashpd::desktop::global_shortcuts::{
    BindShortcutsOptions, GlobalShortcuts, ListShortcutsOptions, NewShortcut,
};
// `StreamExt::next` would need a new direct crate just for one call; poll
// the activation stream by hand through zbus's `futures_core` re-export
// instead (a pinned transitive dependency).
use ashpd::zbus::export::futures_core::Stream as _;

use super::{ShortcutAction, portal_trigger};
use crate::settings::Shortcuts;

/// The four actions with their stable portal ids (the desktop remembers
/// granted bindings per id) and the descriptions shown in its shortcut UI.
const ACTIONS: [(ShortcutAction, &str, &str); 4] = [
    (
        ShortcutAction::ReadOrPause,
        "read_or_pause",
        "Read the clipboard or pause",
    ),
    (ShortcutAction::Stop, "stop", "Stop reading"),
    (ShortcutAction::Next, "next", "Next sentence"),
    (ShortcutAction::Previous, "previous", "Previous sentence"),
];

/// Bind the shortcuts from `shortcuts` through the portal and forward every
/// activation to `activated`. Blocks until something fails (no portal on the
/// desktop, denied dialog, lost bus); the returned message fits the status
/// line and the app keeps running on the tray icon.
pub fn run(shortcuts: &Shortcuts, activated: impl Fn(ShortcutAction)) -> Result<(), String> {
    async_io::block_on(async {
        let texts = [
            &shortcuts.read_or_pause,
            &shortcuts.stop,
            &shortcuts.next,
            &shortcuts.previous,
        ];
        let mut wanted: Vec<(ShortcutAction, &str, NewShortcut)> = Vec::with_capacity(4);
        for ((action, id, description), text) in ACTIONS.iter().zip(texts) {
            // Empty = disabled, skipped silently like on the X11 path.
            let Some(trigger) = portal_trigger(text) else {
                continue;
            };
            let shortcut = NewShortcut::new(*id, *description).preferred_trigger(trigger.as_str());
            wanted.push((*action, *id, shortcut));
        }
        if wanted.is_empty() {
            return Ok(());
        }
        let portal = GlobalShortcuts::new().await.map_err(portal_error)?;
        let session = portal
            .create_session(CreateSessionOptions::default())
            .await
            .map_err(portal_error)?;
        // ashpd 0.13 can't pin the session handle token, so check first
        // which ids the desktop already granted and skip re-binding them.
        let listed = portal
            .list_shortcuts(&session, ListShortcutsOptions::default())
            .await
            .map_err(portal_error)?
            .response()
            .map_err(portal_error)?;
        let bound: HashSet<&str> = listed.shortcuts().iter().map(|s| s.id()).collect();
        let missing: Vec<NewShortcut> = wanted
            .iter()
            .filter(|(_, id, _)| !bound.contains(*id))
            .map(|(_, _, shortcut)| shortcut.clone())
            .collect();
        if !missing.is_empty() {
            portal
                .bind_shortcuts(&session, &missing, None, BindShortcutsOptions::default())
                .await
                .map_err(portal_error)?
                .response()
                .map_err(portal_error)?;
        }
        let mut stream = Box::pin(portal.receive_activated().await.map_err(portal_error)?);
        loop {
            let Some(activation) = poll_fn(|cx| stream.as_mut().poll_next(cx)).await else {
                return Err(portal_error("the shortcut listener stopped unexpectedly"));
            };
            if let Some((action, _, _)) = wanted
                .iter()
                .find(|(_, id, _)| *id == activation.shortcut_id())
            {
                activated(*action);
            }
        }
    })
}

/// Every portal failure surfaces as one message: shortcuts stay unavailable
/// on this desktop and the tray icon remains the way to control the app.
fn portal_error(err: impl std::fmt::Display) -> String {
    format!("Global shortcuts aren't available on this desktop: {err}. Use the tray icon instead.")
}
