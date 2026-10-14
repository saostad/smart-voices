//! Global shortcut helpers: display formatting for the bindings stored in
//! `Settings::shortcuts` (`HotKey::from_str` syntax, e.g. `"ctrl+alt+Space"`).

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
}
