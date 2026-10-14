//! Launch at Login: start the app automatically at login.
//!
//! The Settings menu shows a check item reflecting the real OS state (read
//! fresh every time, never stored); toggling it writes or removes the OS
//! login item: a LaunchAgent plist on macOS, a `Run` registry value on
//! Windows, an autostart `.desktop` file on Linux.

use std::fmt;
use std::path::{Path, PathBuf};

/// Where launch-at-login state lives, plus the binary it points at.
/// [`AutostartPaths::system`] resolves both from the real environment; tests
/// build one pointing at a temp dir so they never touch the real home.
#[derive(Debug, Clone)]
pub struct AutostartPaths {
    /// The login-item file: `~/Library/LaunchAgents/com.smartvoices.rust.plist`
    /// on macOS, `$XDG_CONFIG_HOME/autostart/smart-voices.desktop` (or
    /// `~/.config/autostart/…`) on Linux. Unused on Windows (the registry
    /// holds the state).
    pub file: PathBuf,
    /// The binary to start at login (`std::env::current_exe` in production).
    pub exe: PathBuf,
}

impl AutostartPaths {
    /// Resolve from the real home dir and the running binary.
    pub fn system() -> Result<Self, AutostartError> {
        let exe = std::env::current_exe().map_err(|err| AutostartError {
            message: format!("couldn't find the running binary: {err}"),
        })?;
        #[cfg(target_os = "macos")]
        let file = {
            let home = dirs::home_dir().ok_or_else(|| AutostartError {
                message: "no home directory is available".to_string(),
            })?;
            plist_path(&home)
        };
        #[cfg(target_os = "windows")]
        let file = exe.clone();
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let file = {
            // `dirs::config_dir` respects `$XDG_CONFIG_HOME`.
            let config = dirs::config_dir().ok_or_else(|| AutostartError {
                message: "no config directory is available".to_string(),
            })?;
            desktop_file_path(&config)
        };
        Ok(Self { file, exe })
    }
}

/// Failure to resolve the home/binary or to write/remove the login item.
/// Displays as `Couldn't change Launch at Login: <detail>`, ready for the
/// status line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutostartError {
    message: String,
}

impl fmt::Display for AutostartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Couldn't change Launch at Login: {}", self.message)
    }
}

impl std::error::Error for AutostartError {}

/// Whether launch at login is currently enabled: the real OS state, read
/// fresh (a failure to read counts as disabled).
pub fn is_enabled() -> bool {
    AutostartPaths::system().is_ok_and(|paths| is_enabled_in(&paths))
}

/// Enable or disable launch at login.
pub fn set_enabled(enabled: bool) -> Result<(), AutostartError> {
    let paths = AutostartPaths::system()?;
    set_enabled_in(&paths, enabled)
}

/// Whether launch at login is enabled at `paths` (login-item file presence
/// on macOS/Linux, registry value on Windows).
pub fn is_enabled_in(paths: &AutostartPaths) -> bool {
    #[cfg(target_os = "windows")]
    {
        let _ = paths;
        is_registry_enabled()
    }
    #[cfg(not(target_os = "windows"))]
    {
        paths.file.is_file()
    }
}

/// Enable or disable launch at login at `paths`.
pub fn set_enabled_in(paths: &AutostartPaths, enabled: bool) -> Result<(), AutostartError> {
    #[cfg(target_os = "windows")]
    {
        set_registry_enabled(&paths.exe, enabled)
    }
    #[cfg(target_os = "macos")]
    {
        set_file_enabled(&paths.file, &plist_text(&paths.exe), enabled)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        set_file_enabled(&paths.file, &desktop_file_text(&paths.exe), enabled)
    }
}

/// Login-item file for a home dir (macOS).
#[cfg(target_os = "macos")]
fn plist_path(home: &Path) -> PathBuf {
    home.join("Library/LaunchAgents/com.smartvoices.rust.plist")
}

/// Autostart file for a config dir (Linux).
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn desktop_file_path(config_dir: &Path) -> PathBuf {
    config_dir.join("autostart/smart-voices.desktop")
}

/// Write (`enabled`) or remove the login-item file, creating parent dirs.
/// The write goes through a temp file plus rename so a crash can't leave a
/// half-written item; removing a missing file is a no-op.
#[cfg(not(target_os = "windows"))]
fn set_file_enabled(file: &Path, text: &str, enabled: bool) -> Result<(), AutostartError> {
    if !enabled {
        match std::fs::remove_file(file) {
            Ok(()) => return Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) => {
                return Err(AutostartError {
                    message: format!("couldn't remove {}: {err}", file.display()),
                });
            }
        }
    }
    if let Some(parent) = file.parent()
        && let Err(err) = std::fs::create_dir_all(parent)
    {
        return Err(AutostartError {
            message: format!("couldn't create {}: {err}", parent.display()),
        });
    }
    let tmp = file.with_extension("tmp");
    if let Err(err) = std::fs::write(&tmp, text) {
        return Err(AutostartError {
            message: format!("couldn't write {}: {err}", file.display()),
        });
    }
    if let Err(err) = std::fs::rename(&tmp, file) {
        let _ = std::fs::remove_file(&tmp);
        return Err(AutostartError {
            message: format!("couldn't write {}: {err}", file.display()),
        });
    }
    Ok(())
}

/// LaunchAgent plist text pointing at `exe` (macOS). Pure, tested on all OSes.
pub fn plist_text(exe: &Path) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \t<key>Label</key>\n\
         \t<string>com.smartvoices.rust</string>\n\
         \t<key>ProgramArguments</key>\n\
         \t<array>\n\
         \t\t<string>{}</string>\n\
         \t</array>\n\
         \t<key>RunAtLoad</key>\n\
         \t<true/>\n\
         \t<key>ProcessType</key>\n\
         \t<string>Interactive</string>\n\
         </dict>\n\
         </plist>\n",
        xml_escape(&exe.to_string_lossy())
    )
}

/// Minimal XML escaping for the exe path inside the plist.
fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Autostart `.desktop` text pointing at `exe`: the fields from
/// `packaging/linux/smart-voices.desktop` with a real `Exec` (Linux). Pure,
/// tested on all OSes.
pub fn desktop_file_text(exe: &Path) -> String {
    let exec = exe.to_string_lossy();
    let exec = if exec.chars().any(char::is_whitespace) {
        format!("\"{exec}\"")
    } else {
        exec.into_owned()
    };
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Smart Voices\n\
         Comment=Read copied text aloud\n\
         Exec={exec}\n\
         Icon=audio-volume-high\n\
         Categories=Utility;Accessibility;\n\
         Terminal=false\n\
         X-GNOME-Autostart-enabled=true\n"
    )
}

/// `Run` registry value text: the exe path, quoted (Windows). Pure, tested
/// on all OSes.
pub fn run_value_text(exe: &Path) -> String {
    format!("\"{}\"", exe.display())
}

#[cfg(target_os = "windows")]
const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
#[cfg(target_os = "windows")]
const RUN_VALUE_NAME: &str = "Smart Voices";

/// Whether the `Run` value exists (Windows).
#[cfg(target_os = "windows")]
fn is_registry_enabled() -> bool {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_READ, RegCloseKey, RegOpenKeyExW, RegQueryValueExW,
    };
    // SAFETY: all pointers are valid (NUL-terminated key/name buffers, null
    // for the outputs we don't need); the opened key is always closed.
    unsafe {
        let subkey = wide(RUN_KEY);
        let mut key: HKEY = std::ptr::null_mut();
        if RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, KEY_READ, &mut key) != ERROR_SUCCESS
        {
            return false;
        }
        let name = wide(RUN_VALUE_NAME);
        let found = RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        ) == ERROR_SUCCESS;
        RegCloseKey(key);
        found
    }
}

/// Write or delete the `Run` value (Windows). Deleting a missing value is a
/// no-op.
#[cfg(target_os = "windows")]
fn set_registry_enabled(exe: &Path, enabled: bool) -> Result<(), AutostartError> {
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ, RegCloseKey, RegDeleteValueW,
        RegOpenKeyExW, RegSetValueExW,
    };
    // SAFETY: as in `is_registry_enabled`; the value buffer outlives the
    // `RegSetValueExW` call and its byte length includes the NUL.
    unsafe {
        let subkey = wide(RUN_KEY);
        let mut key: HKEY = std::ptr::null_mut();
        let status = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            0,
            KEY_SET_VALUE,
            &mut key,
        );
        if status != ERROR_SUCCESS {
            return Err(registry_error("couldn't open the Run key", status));
        }
        let name = wide(RUN_VALUE_NAME);
        let result = if enabled {
            let value: Vec<u16> = run_value_text(exe)
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            let status = RegSetValueExW(
                key,
                name.as_ptr(),
                0,
                REG_SZ,
                value.as_ptr() as *const u8,
                (value.len() * 2) as u32,
            );
            if status == ERROR_SUCCESS {
                Ok(())
            } else {
                Err(registry_error("couldn't write the Run value", status))
            }
        } else {
            let status = RegDeleteValueW(key, name.as_ptr());
            if status == ERROR_SUCCESS || status == ERROR_FILE_NOT_FOUND {
                Ok(())
            } else {
                Err(registry_error("couldn't delete the Run value", status))
            }
        };
        RegCloseKey(key);
        result
    }
}

#[cfg(target_os = "windows")]
fn registry_error(step: &str, code: u32) -> AutostartError {
    let detail = match os_error_text(code) {
        Some(text) => format!("{text} (code {code})"),
        None => format!("Windows error {code}"),
    };
    AutostartError {
        message: format!("{step}: {detail}"),
    }
}

#[cfg(target_os = "windows")]
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Render a Win32 error code as text (`FormatMessageW`); `None` when the
/// system has no message for the code.
#[cfg(target_os = "windows")]
fn os_error_text(code: u32) -> Option<String> {
    use windows_sys::Win32::System::Diagnostics::Debug::{
        FORMAT_MESSAGE_FROM_SYSTEM, FORMAT_MESSAGE_IGNORE_INSERTS, FormatMessageW,
    };
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

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(not(target_os = "windows"))]
    use std::sync::atomic::{AtomicU64, Ordering};

    // The file-based round-trip tests that use these don't run on Windows.
    #[cfg(not(target_os = "windows"))]
    static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[cfg(not(target_os = "windows"))]
    fn unique_temp_dir() -> PathBuf {
        let id = TEST_DIR_COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!(
            "smart-voices-autostart-test-{}-{id}",
            std::process::id()
        ))
    }

    #[test]
    fn plist_text_has_the_launchd_keys() {
        let exe = Path::new("/Applications/Smart Voices.app/Contents/MacOS/smart-voices");
        assert_eq!(
            plist_text(exe),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
             \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n\
             <dict>\n\
             \t<key>Label</key>\n\
             \t<string>com.smartvoices.rust</string>\n\
             \t<key>ProgramArguments</key>\n\
             \t<array>\n\
             \t\t<string>/Applications/Smart Voices.app/Contents/MacOS/smart-voices</string>\n\
             \t</array>\n\
             \t<key>RunAtLoad</key>\n\
             \t<true/>\n\
             \t<key>ProcessType</key>\n\
             \t<string>Interactive</string>\n\
             </dict>\n\
             </plist>\n"
        );
    }

    #[test]
    fn plist_text_escapes_xml() {
        let exe = Path::new("/tmp/A&B<C>/smart-voices");
        let text = plist_text(exe);
        assert!(
            text.contains("<string>/tmp/A&amp;B&lt;C&gt;/smart-voices</string>"),
            "{text}"
        );
    }

    #[test]
    fn desktop_file_text_matches_the_packaging_fields() {
        let exe = Path::new("/home/ann/.local/bin/smart-voices");
        assert_eq!(
            desktop_file_text(exe),
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=Smart Voices\n\
             Comment=Read copied text aloud\n\
             Exec=/home/ann/.local/bin/smart-voices\n\
             Icon=audio-volume-high\n\
             Categories=Utility;Accessibility;\n\
             Terminal=false\n\
             X-GNOME-Autostart-enabled=true\n"
        );
    }

    #[test]
    fn desktop_file_text_quotes_paths_with_spaces() {
        let exe = Path::new("/home/ann/My Apps/smart-voices");
        let text = desktop_file_text(exe);
        assert!(
            text.contains("Exec=\"/home/ann/My Apps/smart-voices\"\n"),
            "{text}"
        );
    }

    #[test]
    fn run_value_text_is_quoted() {
        assert_eq!(
            run_value_text(Path::new("C:\\Tools\\smart-voices.exe")),
            "\"C:\\Tools\\smart-voices.exe\""
        );
    }

    #[test]
    fn error_display_fits_the_status_line() {
        let err = AutostartError {
            message: "couldn't write /x: denied".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "Couldn't change Launch at Login: couldn't write /x: denied"
        );
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn enable_reports_an_uncreatable_parent() {
        // Point the file into a path whose parent can't be created: use a
        // regular file as the parent.
        let dir = unique_temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let blocker = dir.join("blocker");
        std::fs::write(&blocker, "x").unwrap();
        let bad = AutostartPaths {
            file: blocker.join("smart-voices.desktop"),
            exe: PathBuf::from("/usr/bin/false"),
        };
        let err = set_enabled_in(&bad, true).unwrap_err();
        assert!(
            err.to_string()
                .starts_with("Couldn't change Launch at Login: couldn't create "),
            "{err}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn plist_round_trip_under_a_temp_home() {
        let dir = unique_temp_dir();
        let paths = AutostartPaths {
            file: plist_path(&dir),
            exe: PathBuf::from("/Applications/Smart Voices.app/Contents/MacOS/smart-voices"),
        };
        assert!(!is_enabled_in(&paths));
        set_enabled_in(&paths, true).unwrap();
        assert!(is_enabled_in(&paths));
        assert_eq!(
            std::fs::read_to_string(&paths.file).unwrap(),
            plist_text(&paths.exe)
        );
        set_enabled_in(&paths, false).unwrap();
        assert!(!is_enabled_in(&paths));
        assert!(!paths.file.exists());
        // Disabling twice is a no-op.
        set_enabled_in(&paths, false).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn desktop_file_round_trip_under_a_temp_config_home() {
        let dir = unique_temp_dir();
        let paths = AutostartPaths {
            file: desktop_file_path(&dir),
            exe: PathBuf::from("/home/ann/.local/bin/smart-voices"),
        };
        assert!(!is_enabled_in(&paths));
        set_enabled_in(&paths, true).unwrap();
        assert!(is_enabled_in(&paths));
        assert_eq!(
            std::fs::read_to_string(&paths.file).unwrap(),
            desktop_file_text(&paths.exe)
        );
        set_enabled_in(&paths, false).unwrap();
        assert!(!is_enabled_in(&paths));
        assert!(!paths.file.exists());
        // Disabling twice is a no-op.
        set_enabled_in(&paths, false).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The file round-trip through the shared helper, on every OS (the
    /// helpers are file-based on macOS/Linux; this one pins the helper
    /// itself so Windows CI covers the pure parts too).
    #[test]
    #[cfg(not(target_os = "windows"))]
    fn file_round_trip_covers_both_texts() {
        for (name, text) in [
            ("com.smartvoices.rust.plist", plist_text(Path::new("/x/sv"))),
            (
                "smart-voices.desktop",
                desktop_file_text(Path::new("/x/sv")),
            ),
        ] {
            let dir = unique_temp_dir();
            let file = dir.join("sub").join(name);
            assert!(!file.is_file());
            set_file_enabled(&file, &text, true).unwrap();
            assert!(file.is_file());
            assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
            set_file_enabled(&file, &text, false).unwrap();
            assert!(!file.exists());
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }
}
