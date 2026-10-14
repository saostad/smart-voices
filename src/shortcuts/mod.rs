//! Global shortcut helpers: parsing and display formatting for the bindings
//! stored in `Settings::shortcuts` (`HotKey::from_str` syntax, e.g.
//! `"ctrl+alt+Space"`).

#[cfg(target_os = "linux")]
pub mod portal;

use global_hotkey::hotkey::HotKey;

/// What a registered global shortcut does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutAction {
    /// Read the clipboard when idle, pause/resume while reading.
    ReadOrPause,
    /// Stop reading.
    Stop,
    /// Jump to the next chunk.
    Next,
    /// Jump to the previous chunk.
    Previous,
}

/// Parse one binding: an empty string means disabled (`Ok(None)`), anything
/// else must be `HotKey::from_str` syntax.
pub fn parse_binding(text: &str) -> Result<Option<HotKey>, String> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    text.parse::<HotKey>()
        .map(Some)
        .map_err(|err| err.to_string())
}

/// Format a shortcut binding for the menu (`"ctrl+alt+Space"` →
/// `"Ctrl+Alt+Space"`, `"⌃⌥Space"` on macOS). An empty binding (disabled)
/// shows as `"Off"`. Unknown tokens are shown as-is so a typo is visible
/// rather than silently dropped.
pub fn format_binding(binding: &str) -> String {
    if binding.trim().is_empty() {
        return "Off".to_string();
    }
    let mut mods: Vec<&str> = Vec::new();
    let mut key: Option<&str> = None;
    for token in binding.split('+') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        match token.to_uppercase().as_str() {
            "CTRL" | "CONTROL" => push_unique(&mut mods, "ctrl"),
            "ALT" | "OPTION" => push_unique(&mut mods, "alt"),
            "SHIFT" => push_unique(&mut mods, "shift"),
            "SUPER" | "CMD" | "COMMAND" | "META" => push_unique(&mut mods, "super"),
            "COMMANDORCONTROL" | "COMMANDORCTRL" | "CMDORCTRL" | "CMDORCONTROL" => {
                push_unique(&mut mods, command_or_control())
            }
            _ => key = Some(token),
        }
    }
    // Canonical modifier order, whatever order the file used.
    mods.sort_by_key(|name| match *name {
        "ctrl" => 0,
        "alt" => 1,
        "shift" => 2,
        _ => 3,
    });
    let key = key.map(format_key).unwrap_or_else(|| binding.to_string());
    #[cfg(target_os = "macos")]
    {
        let glyphs: String = mods.iter().map(|name| mod_glyph(name)).collect();
        format!("{glyphs}{key}")
    }
    #[cfg(not(target_os = "macos"))]
    {
        let mut parts: Vec<&str> = mods.iter().map(|name| mod_label(name)).collect();
        parts.push(&key);
        parts.join("+")
    }
}

fn push_unique<'a>(mods: &mut Vec<&'a str>, name: &'a str) {
    if !mods.contains(&name) {
        mods.push(name);
    }
}

#[cfg(target_os = "macos")]
fn command_or_control() -> &'static str {
    "super"
}

#[cfg(not(target_os = "macos"))]
fn command_or_control() -> &'static str {
    "ctrl"
}

#[cfg(target_os = "macos")]
fn mod_glyph(name: &str) -> &'static str {
    match name {
        "ctrl" => "\u{2303}",  // ⌃
        "alt" => "\u{2325}",   // ⌥
        "shift" => "\u{21e7}", // ⇧
        _ => "\u{2318}",       // ⌘
    }
}

#[cfg(not(target_os = "macos"))]
fn mod_label(name: &str) -> &'static str {
    match name {
        "ctrl" => "Ctrl",
        "alt" => "Alt",
        "shift" => "Shift",
        _ => "Super",
    }
}

/// Convert a settings binding (`"ctrl+alt+Space"`, `HotKey::from_str`
/// syntax) to the XDG GlobalShortcuts portal trigger syntax
/// (`"CTRL+ALT+space"`). Returns `None` for an empty (disabled) binding.
/// Unknown key names pass through unchanged; the portal then rejects the
/// bind and it surfaces as a portal error.
pub fn portal_trigger(binding: &str) -> Option<String> {
    if binding.trim().is_empty() {
        return None;
    }
    let mut mods: Vec<&str> = Vec::new();
    let mut key: Option<&str> = None;
    for token in binding.split('+') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        match token.to_uppercase().as_str() {
            "CTRL" | "CONTROL" | "COMMANDORCONTROL" | "COMMANDORCTRL" | "CMDORCTRL"
            | "CMDORCONTROL" => push_unique(&mut mods, "CTRL"),
            "ALT" | "OPTION" => push_unique(&mut mods, "ALT"),
            "SHIFT" => push_unique(&mut mods, "SHIFT"),
            "SUPER" | "META" | "CMD" | "COMMAND" => push_unique(&mut mods, "SUPER"),
            _ => key = Some(token),
        }
    }
    // Canonical modifier order, whatever order the file used.
    mods.sort_by_key(|name| match *name {
        "CTRL" => 0,
        "ALT" => 1,
        "SHIFT" => 2,
        _ => 3,
    });
    let mut parts: Vec<String> = mods.iter().map(ToString::to_string).collect();
    match key {
        Some(token) => parts.push(portal_key(token)),
        // No key part at all (e.g. `"ctrl+alt"`): pass the raw binding
        // through so the portal rejects it loudly instead of binding
        // something unexpected.
        None => parts.push(binding.trim().to_string()),
    }
    Some(parts.join("+"))
}

/// Map the main key to its portal trigger name: `Space` → `space`, `Period`
/// → `period`, arrows → `Right`/`Left`/`Up`/`Down`, single characters
/// lowercased, function keys uppercased; anything else passes through.
fn portal_key(token: &str) -> String {
    match token.to_uppercase().as_str() {
        "SPACE" => "space".to_string(),
        "PERIOD" | "." => "period".to_string(),
        "COMMA" | "," => "comma".to_string(),
        "SLASH" | "/" => "slash".to_string(),
        "SEMICOLON" | ";" => "semicolon".to_string(),
        "MINUS" | "-" => "minus".to_string(),
        "EQUAL" | "=" => "equal".to_string(),
        "ARROWRIGHT" | "RIGHT" => "Right".to_string(),
        "ARROWLEFT" | "LEFT" => "Left".to_string(),
        "ARROWUP" | "UP" => "Up".to_string(),
        "ARROWDOWN" | "DOWN" => "Down".to_string(),
        "ESCAPE" | "ESC" => "Escape".to_string(),
        "TAB" => "Tab".to_string(),
        "BACKSPACE" => "BackSpace".to_string(),
        "RETURN" | "ENTER" => "Return".to_string(),
        "DELETE" | "DEL" => "Delete".to_string(),
        "HOME" => "Home".to_string(),
        "END" => "End".to_string(),
        "PAGEUP" => "Page_Up".to_string(),
        "PAGEDOWN" => "Page_Down".to_string(),
        "INSERT" | "INS" => "Insert".to_string(),
        _ if token.chars().count() == 1 => token.to_lowercase(),
        _ if is_function_key(token) => token.to_uppercase(),
        _ => token.to_string(),
    }
}

/// `F1`–`F24`, case-insensitive. The `[1..]` slice is safe: the token starts
/// with ASCII `F`/`f`, so byte 1 is a char boundary (and `"F"[1..]` is just
/// empty, which fails the number parse).
fn is_function_key(token: &str) -> bool {
    token.len() <= 3
        && token.starts_with(['F', 'f'])
        && token[1..]
            .parse::<u8>()
            .is_ok_and(|n| (1..=24).contains(&n))
}

/// Display text for the main key: punctuation and arrows become glyphs,
/// single characters are uppercased, other names are shown as-is.
fn format_key(token: &str) -> String {
    match token.to_uppercase().as_str() {
        "SPACE" => "Space".to_string(),
        "PERIOD" | "." => ".".to_string(),
        "COMMA" | "," => ",".to_string(),
        "SLASH" | "/" => "/".to_string(),
        "SEMICOLON" | ";" => ";".to_string(),
        "MINUS" | "-" => "-".to_string(),
        "EQUAL" | "=" => "=".to_string(),
        "ARROWRIGHT" | "RIGHT" => "\u{2192}".to_string(), // →
        "ARROWLEFT" | "LEFT" => "\u{2190}".to_string(),   // ←
        "ARROWUP" | "UP" => "\u{2191}".to_string(),       // ↑
        "ARROWDOWN" | "DOWN" => "\u{2193}".to_string(),   // ↓
        _ if token.chars().count() == 1 => token.to_uppercase(),
        _ => token.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_defaults_and_rejects_garbage() {
        use crate::settings::Shortcuts;
        let defaults = Shortcuts::default();
        for binding in [
            &defaults.read_or_pause,
            &defaults.stop,
            &defaults.next,
            &defaults.previous,
        ] {
            assert!(parse_binding(binding).unwrap().is_some(), "{binding}");
        }
        assert_eq!(parse_binding(""), Ok(None));
        assert_eq!(parse_binding("   "), Ok(None));
        assert!(parse_binding("ctrl+alt+frobnicate").is_err());
        assert!(parse_binding("ctrl+Space+alt").is_err());
    }

    #[test]
    fn parsed_ids_are_stable() {
        // Event routing matches on `HotKey::id`, which must be deterministic.
        let first = parse_binding("ctrl+alt+Space").unwrap().unwrap();
        let second = parse_binding("ctrl+alt+Space").unwrap().unwrap();
        assert_eq!(first.id(), second.id());
    }

    #[test]
    fn empty_binding_shows_off() {
        assert_eq!(format_binding(""), "Off");
        assert_eq!(format_binding("   "), "Off");
    }

    #[test]
    fn formats_default_bindings() {
        #[cfg(target_os = "macos")]
        {
            assert_eq!(format_binding("ctrl+alt+Space"), "\u{2303}\u{2325}Space");
            assert_eq!(format_binding("ctrl+alt+Period"), "\u{2303}\u{2325}.");
            assert_eq!(
                format_binding("ctrl+alt+ArrowRight"),
                "\u{2303}\u{2325}\u{2192}"
            );
            assert_eq!(
                format_binding("ctrl+alt+ArrowLeft"),
                "\u{2303}\u{2325}\u{2190}"
            );
        }
        #[cfg(not(target_os = "macos"))]
        {
            assert_eq!(format_binding("ctrl+alt+Space"), "Ctrl+Alt+Space");
            assert_eq!(format_binding("ctrl+alt+Period"), "Ctrl+Alt+.");
            assert_eq!(format_binding("ctrl+alt+ArrowRight"), "Ctrl+Alt+\u{2192}");
            assert_eq!(format_binding("ctrl+alt+ArrowLeft"), "Ctrl+Alt+\u{2190}");
        }
    }

    #[test]
    fn modifier_order_is_canonical() {
        let formatted = format_binding("alt+ctrl+q");
        #[cfg(target_os = "macos")]
        assert_eq!(formatted, "\u{2303}\u{2325}Q");
        #[cfg(not(target_os = "macos"))]
        assert_eq!(formatted, "Ctrl+Alt+Q");
    }

    #[test]
    fn unknown_tokens_are_shown() {
        let formatted = format_binding("ctrl+ frobnicate ");
        #[cfg(target_os = "macos")]
        assert_eq!(formatted, "\u{2303}frobnicate");
        #[cfg(not(target_os = "macos"))]
        assert_eq!(formatted, "Ctrl+frobnicate");
    }

    #[test]
    fn portal_triggers_for_the_default_bindings() {
        assert_eq!(
            portal_trigger("ctrl+alt+Space"),
            Some("CTRL+ALT+space".to_string())
        );
        assert_eq!(
            portal_trigger("ctrl+alt+Period"),
            Some("CTRL+ALT+period".to_string())
        );
        assert_eq!(
            portal_trigger("ctrl+alt+ArrowRight"),
            Some("CTRL+ALT+Right".to_string())
        );
        assert_eq!(
            portal_trigger("ctrl+alt+ArrowLeft"),
            Some("CTRL+ALT+Left".to_string())
        );
    }

    #[test]
    fn portal_trigger_rules() {
        assert_eq!(portal_trigger(""), None);
        assert_eq!(portal_trigger("   "), None);
        // Canonical modifier order, whatever order the file used.
        assert_eq!(portal_trigger("alt+ctrl+q"), Some("CTRL+ALT+q".to_string()));
        assert_eq!(
            portal_trigger("super+shift+alt+ctrl+F5"),
            Some("CTRL+ALT+SHIFT+SUPER+F5".to_string())
        );
        // Single characters are lowercased, digits pass through.
        assert_eq!(portal_trigger("ctrl+Q"), Some("CTRL+q".to_string()));
        assert_eq!(portal_trigger("ctrl+5"), Some("CTRL+5".to_string()));
        // Named keys.
        assert_eq!(
            portal_trigger("ctrl+Escape"),
            Some("CTRL+Escape".to_string())
        );
        assert_eq!(portal_trigger("alt+F12"), Some("ALT+F12".to_string()));
        // Unknown key names pass through for the portal to reject.
        assert_eq!(
            portal_trigger("ctrl+frobnicate"),
            Some("CTRL+frobnicate".to_string())
        );
        // No key part: the raw binding passes through.
        assert_eq!(
            portal_trigger("ctrl+alt"),
            Some("CTRL+ALT+ctrl+alt".to_string())
        );
    }
}
