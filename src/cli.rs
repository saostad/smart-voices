//! CLI services. Settings serialization provides one source of truth for keys.
use crate::settings::Settings;
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{IsTerminal, Write};
use std::path::Path;

/// Subcommands that act on a running tray app or `serve`.
pub const CONTROL_COMMANDS: &[&str] = &[
    "play",
    "read-clipboard",
    "pause",
    "resume",
    "toggle",
    "stop",
    "next",
    "previous",
    "jump",
    "status",
    "show-panel",
    "quit",
];

pub const HELP: &str = r#"Usage:
  smart-voices                         Start tray app
  smart-voices --version               Print package version
  smart-voices synth [TEXT|-] --out PATH|- [--format FORMAT] [--file PATH]
                     [--float] [--sample-rate HZ] [--SETTING VALUE]...
                     [--set dotted.key VALUE]...
  smart-voices serve [--listen ADDR:PORT] [--token-file PATH]
                     [--max-concurrency N] [--max-queue N] [--max-chars N]
                     [--max-body-bytes N] [--idle-unload-secs N] [--max-handlers N]
                     [--read-timeout-secs N] [--max-header-bytes N] [--accept-poll-ms N]
                     [--linger-drain-bytes N] [--linger-secs N] [--max-lingering-rejections N]
  smart-voices play TEXT|-|--file PATH  Read aloud on the running tray or serve
  smart-voices read-clipboard          Read the clipboard, like the tray's Read Clipboard
  smart-voices pause|resume|toggle|stop|next|previous
  smart-voices jump N                  Go to chunk N (from 1), like clicking in the panel
  smart-voices status [--json]         What is reading now, and how far along
  smart-voices show-panel              Open the reading panel (tray only)
  smart-voices quit                    Quit the running tray or serve
  smart-voices speak TEXT              Play through speakers (existing flags)
  smart-voices speak TEXT --out PATH   Same as synth
  smart-voices config list [--json]
  smart-voices config get KEY
  smart-voices config set KEY VALUE
  smart-voices config reset [KEY]
  smart-voices config path
  smart-voices voices [--model fast|small|phonon|paradee|zonos]
  smart-voices download [--model MODEL] [--persian] [--persian-voice ID]
  smart-voices clean [FILE]
  smart-voices watch [--auto-read]
  smart-voices reader-preview OUT.bmp [--dark] [--width N] [--height N] [--scale N]

TEXT may be omitted when stdin is piped. Use -- before text starting with --.
Formats: wav (signed PCM16, or --float for float32), pcm/raw (little-endian
mono PCM16, or --float), flac, mp3, ogg/opus (Ogg Opus).
FLAC/MP3/Opus require ffmpeg on PATH with flac/libmp3lame/libopus encoders.
Format is inferred from extension; stdout requires --format. No mismatches.
Output defaults to the first chunk's sample rate; --sample-rate overrides it.

serve defaults (also server.* settings): listen=127.0.0.1:7341, token_file=""
(auth off), max_concurrency=1, max_queue=4, max_chars=20000,
max_body_bytes=1048576, idle_unload_secs=1200 (0=never), max_handlers=16,
read_timeout_secs=10, max_header_bytes=16384, accept_poll_ms=10,
linger_drain_bytes=1048576, linger_secs=2, max_lingering_rejections=4.
Without a token, anyone who can reach the port can use the API.
GET /v1/health, GET /v1/capabilities, POST /v1/speak (JSON). The control routes
/v1/playback, /v1/settings, /v1/pronunciations, /v1/autostart, /v1/downloads,
/v1/panel and /v1/quit do what the tray menu does; see the README.
The tray app serves the same API. Control commands use a live tray first, then
serve; SMART_VOICES_URL and SMART_VOICES_TOKEN point them elsewhere.
Prefer localhost/Tailscale. Requests never download models.

Every Settings key is accepted as --key-name VALUE or --set dotted.key VALUE
for this invocation only. Run config list for keys/current values and allowed
voice/model values. Speed range: 0.5..2; booleans: true/false.
Additional config keys: autostart (OS login item), pronunciations (JSON array
of {written, spoken, case_sensitive, whole_word}); pronunciations.path is read-only.
config reset without a key resets settings.toml only. Defaults apply when a
settings file is absent. SMART_VOICES_CONFIG selects an alternate settings file.
SMART_VOICES_MODELS selects model storage. synth requires installed models;
use download explicitly. Saved settings apply right away on a running tray or
serve, except server.* and shortcuts.*, which need a restart; pronunciations
reload at each reading, autostart changes the OS login item immediately.
"#;

fn flatten(value: &Value, prefix: &str, out: &mut BTreeMap<String, Value>) {
    if let Value::Object(map) = value {
        for (key, value) in map {
            let key = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            flatten(value, &key, out);
        }
    } else {
        out.insert(prefix.into(), value.clone());
    }
}
pub fn values(settings: &Settings) -> Result<BTreeMap<String, Value>, String> {
    let mut out = BTreeMap::new();
    flatten(
        &serde_json::to_value(settings).map_err(|e| e.to_string())?,
        "",
        &mut out,
    );
    Ok(out)
}
pub fn set(settings: &mut Settings, key: &str, text: &str) -> Result<(), String> {
    let mut value = serde_json::to_value(&*settings).map_err(|e| e.to_string())?;
    let mut slot = &mut value;
    for part in key.split('.') {
        slot = slot
            .get_mut(part)
            .ok_or_else(|| format!("unknown settings key '{key}'"))?;
    }
    if slot.is_object() {
        return Err("select an individual dotted key".into());
    }
    *slot = if slot.is_string() {
        Value::String(text.into())
    } else {
        serde_json::from_str(text).map_err(|e| format!("invalid value for {key}: {e}"))?
    };
    let candidate: Settings =
        serde_json::from_value(value).map_err(|e| format!("invalid value for {key}: {e}"))?;
    candidate.validate_strict()?;
    *settings = candidate;
    Ok(())
}
pub fn load(path: &Path) -> Result<Settings, String> {
    let (settings, error) = Settings::load(path);
    if let Some(error) = error {
        return Err(format!(
            "{error}. Fix the file, or run `smart-voices config reset` to restore defaults"
        ));
    }
    Ok(settings)
}

fn catalog(model: Option<&str>) -> Result<(), String> {
    if model.is_none() || matches!(model, Some("fast" | "small")) {
        println!("fast / small: {}", crate::engine::VOICES.join(", "));
    }
    if model.is_none() || model == Some("paradee") {
        println!("paradee: {}", crate::engine::PARADEE_VOICES.join(", "));
    }
    if model.is_none() || model == Some("phonon") {
        println!("phonon: default");
        for voice in crate::phonon::PHONON_VOICES {
            println!("  {} ({})", voice.id, voice.name);
        }
    }
    if model.is_none() || model == Some("zonos") {
        println!("zonos:");
        for voice in crate::zonos::ZONOS_VOICES {
            println!("  {} — {}", voice.id, voice.label);
        }
    }
    if let Some(model) = model
        && crate::engine::ModelVariant::parse(model).is_none()
    {
        return Err(format!("unknown model '{model}'"));
    }
    println!(
        "persian_voice: {}",
        crate::engine::PERSIAN_VOICES
            .iter()
            .map(|v| v.id())
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok(())
}
pub(crate) fn pronunciations(path: &Path) -> Result<crate::pronounce::Pronunciations, String> {
    if !path
        .try_exists()
        .map_err(|e| format!("couldn't inspect pronunciation file: {e}"))?
    {
        return Ok(crate::pronounce::Pronunciations::with_defaults());
    }
    let (dict, error) = crate::pronounce::Pronunciations::load(path);
    if let Some(error) = error {
        return Err(error.to_string());
    }
    Ok(dict)
}
pub(crate) fn pronunciation_path(path: &Path) -> Result<std::path::PathBuf, String> {
    Ok(path
        .parent()
        .ok_or("settings path has no parent")?
        .join("pronunciations.toml"))
}
/// Validate a JSON array of pronunciation entries and save it.
pub(crate) fn save_pronunciations(path: &Path, json: &str) -> Result<(), String> {
    let entries: Vec<crate::pronounce::PronunciationEntry> =
        serde_json::from_str(json).map_err(|e| format!("invalid pronunciation JSON: {e}"))?;
    if entries
        .iter()
        .any(|e| e.written.trim().is_empty() || e.spoken.trim().is_empty())
    {
        return Err("pronunciation written and spoken text must be nonempty".into());
    }
    crate::pronounce::Pronunciations::new(entries)
        .save(path)
        .map_err(|e| e.to_string())
}

/// Save settings. A running host gets the changed keys so they apply right
/// away; otherwise the file is written for the next start.
fn save_settings(settings: &Settings, path: &Path) -> Result<(), String> {
    let Some(target) = crate::control::discover(path) else {
        settings.save(path).map_err(|e| e.to_string())?;
        eprintln!("Settings saved; they apply the next time Smart Voices starts.");
        return Ok(());
    };
    let before = values(&load(path)?)?;
    let changes: serde_json::Map<String, Value> = values(settings)?
        .into_iter()
        .filter(|(key, value)| before.get(key) != Some(value))
        .collect();
    if changes.is_empty() {
        eprintln!("Settings unchanged.");
        return Ok(());
    }
    let reply = crate::control::call(
        &target,
        "PATCH",
        "/v1/settings",
        Some(&Value::Object(changes)),
    )?;
    let restart = reply["restartRequired"].as_array().map_or(0, Vec::len);
    if restart > 0 {
        eprintln!("Settings saved. Restart Smart Voices to apply server and shortcut changes.");
    } else {
        eprintln!("Settings saved and applied.");
    }
    Ok(())
}

/// The host a control command talks to.
fn control_target(path: &Path) -> Result<crate::control::Target, String> {
    crate::control::discover(path).ok_or_else(|| {
        "Smart Voices isn't running. Start the tray app with `smart-voices` or the service with `smart-voices serve`; `smart-voices speak TEXT` reads without either.".to_string()
    })
}

/// Print a playback status for a person.
fn print_status(status: &Value) {
    println!("state: {}", status["state"].as_str().unwrap_or("unknown"));
    println!("status: {}", status["status"].as_str().unwrap_or(""));
    println!("host: {}", status["host"].as_str().unwrap_or("unknown"));
    if let Some(index) = status["index"].as_u64() {
        println!("chunk: {} of {}", index + 1, status["count"]);
    }
    if let Some(text) = status["text"].as_str() {
        println!("text: {text}");
    }
    if let Some(error) = status["error"].as_str() {
        println!("error: {error}");
    }
}

/// The text of `play TEXT|-` or `play --file PATH`, or piped stdin.
fn play_text(args: &[String]) -> Result<String, String> {
    match args {
        [flag, file] if flag == "--file" => {
            std::fs::read_to_string(file).map_err(|e| format!("couldn't read {file}: {e}"))
        }
        [text] if text != "-" => Ok(text.clone()),
        [] | [_] => {
            if args.is_empty() && std::io::stdin().is_terminal() {
                return Err("usage: play TEXT | play - | play --file PATH".into());
            }
            std::io::read_to_string(std::io::stdin())
                .map_err(|e| format!("couldn't read stdin: {e}"))
        }
        _ => Err("usage: play TEXT | play - | play --file PATH".into()),
    }
}

/// Commands that act on a running host, as the tray menu does.
fn control_command(command: &str, args: &[String], path: &Path) -> Result<(), String> {
    let json_output = args.iter().any(|arg| arg == "--json");
    let no_args = |usage: &str| -> Result<(), String> {
        if args.iter().all(|arg| arg == "--json") {
            Ok(())
        } else {
            Err(format!("usage: {usage}"))
        }
    };
    let (method, route, body) = match command {
        "play" => (
            "POST",
            "/v1/playback",
            serde_json::json!({"text": play_text(args)?}),
        ),
        "read-clipboard" => {
            no_args("read-clipboard")?;
            (
                "POST",
                "/v1/playback",
                serde_json::json!({"clipboard": true}),
            )
        }
        "jump" => {
            let index: u64 = args
                .first()
                .and_then(|n| n.parse().ok())
                .filter(|n| *n > 0)
                .ok_or("usage: jump N (the chunk number, from 1)")?;
            (
                "POST",
                "/v1/playback/jump",
                serde_json::json!({"index": index - 1}),
            )
        }
        "status" => {
            no_args("status [--json]")?;
            ("GET", "/v1/playback", Value::Null)
        }
        "show-panel" => {
            no_args("show-panel")?;
            ("POST", "/v1/panel", serde_json::json!({}))
        }
        "quit" => {
            no_args("quit")?;
            ("POST", "/v1/quit", serde_json::json!({}))
        }
        "pause" | "resume" | "toggle" | "stop" | "next" | "previous" => {
            no_args(command)?;
            let route = match command {
                "pause" => "/v1/playback/pause",
                "resume" => "/v1/playback/resume",
                "toggle" => "/v1/playback/toggle",
                "stop" => "/v1/playback/stop",
                "next" => "/v1/playback/next",
                _ => "/v1/playback/previous",
            };
            ("POST", route, serde_json::json!({}))
        }
        _ => return Err(HELP.into()),
    };
    let target = control_target(path)?;
    let body = (!body.is_null()).then_some(&body);
    let status = crate::control::call(&target, method, route, body)?;
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&status).map_err(|e| e.to_string())?
        );
    } else {
        print_status(&status);
    }
    Ok(())
}

fn config(args: &[String], path: &Path) -> Result<(), String> {
    if args == ["path"] {
        println!("{}", path.display());
        return Ok(());
    }
    let mut settings = if args == ["reset"] {
        Settings::default()
    } else {
        load(path)?
    };
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["path"] => println!("{}", path.display()),
        ["get", "pronunciations.path"] => println!("{}", pronunciation_path(path)?.display()),
        ["get", "autostart"] => println!("{}", crate::autostart::is_enabled()),
        ["get", "pronunciations"] => println!(
            "{}",
            serde_json::to_string_pretty(&pronunciations(&pronunciation_path(path)?)?.entries())
                .map_err(|e| e.to_string())?
        ),
        ["set", "autostart", value] => {
            crate::autostart::set_enabled(
                value
                    .parse::<bool>()
                    .map_err(|_| "autostart must be true or false")?,
            )
            .map_err(|e| e.to_string())?;
            eprintln!("Launch at Login updated.");
        }
        ["reset", "autostart"] => {
            crate::autostart::set_enabled(false).map_err(|e| e.to_string())?;
            eprintln!("Launch at Login disabled.");
        }
        ["set", "pronunciations", value] => {
            save_pronunciations(&pronunciation_path(path)?, value)?;
            eprintln!("Pronunciations saved; tray reloads them at the next reading.");
        }
        ["reset", "pronunciations"] => {
            crate::pronounce::Pronunciations::with_defaults()
                .save(&pronunciation_path(path)?)
                .map_err(|e| e.to_string())?;
            eprintln!("Pronunciations reset; tray reloads them at the next reading.");
        }
        ["list"] | ["list", "--json"] => {
            let mut all = values(&settings)?;
            all.insert(
                "autostart".into(),
                Value::Bool(crate::autostart::is_enabled()),
            );
            all.insert(
                "pronunciations.path".into(),
                Value::String(pronunciation_path(path)?.display().to_string()),
            );
            all.insert(
                "pronunciations".into(),
                serde_json::to_value(pronunciations(&pronunciation_path(path)?)?.entries())
                    .map_err(|e| e.to_string())?,
            );
            if words.len() == 2 {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&all).map_err(|e| e.to_string())?
                );
            } else {
                for (key, value) in all {
                    println!("{key} = {value}");
                }
                catalog(None)?;
                println!("model: fast, small, phonon, paradee");
            }
        }
        ["get", key] => println!(
            "{}",
            values(&settings)?
                .get(*key)
                .ok_or_else(|| format!("unknown settings key '{key}'"))?
        ),
        ["set", key, value] => {
            set(&mut settings, key, value)?;
            save_settings(&settings, path)?;
        }
        ["reset"] | ["reset", _] => {
            if let Some(key) = words.get(1) {
                let defaults = values(&Settings::default())?;
                let value = defaults
                    .get(*key)
                    .ok_or_else(|| format!("unknown settings key '{key}'"))?;
                let text = value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.to_string());
                set(&mut settings, key, &text)?;
            } else {
                settings = Settings::default();
            }
            save_settings(&settings, path)?;
        }
        _ => {
            return Err(
                "usage: config list [--json] | get KEY | set KEY VALUE | reset [KEY] | path".into(),
            );
        }
    }
    Ok(())
}

#[derive(Debug, Default)]
pub struct SynthArgs {
    pub text: Option<String>,
    pub file: Option<String>,
    pub output: String,
    pub format: Option<String>,
    pub float: bool,
    pub rate: Option<u32>,
    pub overrides: Vec<(String, String)>,
}
pub fn parse_synth(args: &[String]) -> Result<SynthArgs, String> {
    let mut parsed = SynthArgs::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut next = || {
            args.next()
                .cloned()
                .ok_or_else(|| format!("{arg} requires a value"))
        };
        match arg.as_str() {
            "--out" | "-o" => parsed.output = next()?,
            "--format" => parsed.format = Some(next()?),
            "--file" => parsed.file = Some(next()?),
            "--float" => parsed.float = true,
            // Accepted for `speak --out` compatibility; export always reports what it wrote.
            "--verbose" => {}
            "--voice" => parsed.overrides.push(("__voice".into(), next()?)),
            "--sample-rate" => {
                let rate = next()?
                    .parse::<u32>()
                    .map_err(|_| "sample rate must be a positive integer")?;
                if rate == 0 {
                    return Err("sample rate must be positive".into());
                }
                parsed.rate = Some(rate);
            }
            "--set" => {
                let key = next()?;
                let value = next()?;
                parsed.overrides.push((key, value));
            }
            "--" => {
                if parsed.text.is_some() {
                    return Err("only one text argument is allowed".into());
                }
                parsed.text = Some(next()?);
                if args.next().is_some() {
                    return Err("only one text argument is allowed".into());
                }
                break;
            }
            flag if flag.starts_with("--") => {
                parsed
                    .overrides
                    .push((flag[2..].replace('-', "_"), next()?));
            }
            text => {
                if parsed.text.replace(text.into()).is_some() {
                    return Err("only one text argument is allowed".into());
                }
            }
        }
    }
    if parsed.output.is_empty() {
        return Err("synth requires --out PATH (or --out -)".into());
    }
    if parsed.file.is_some() && parsed.text.is_some() {
        return Err("--file and positional text are mutually exclusive".into());
    }
    Ok(parsed)
}
fn synth(args: &[String], path: &Path) -> Result<(), String> {
    let options = parse_synth(args)?;
    let format = crate::export::Format::resolve(&options.output, options.format.as_deref())?;
    if options.float
        && !matches!(
            format,
            crate::export::Format::Wav | crate::export::Format::Pcm
        )
    {
        return Err("--float is supported only for WAV and raw PCM".into());
    }
    format.check_encoder()?;
    let mut settings = load(path)?;
    for (key, value) in &options.overrides {
        if key != "__voice" {
            set(&mut settings, key, value)?;
        }
    }
    for (key, value) in &options.overrides {
        if key == "__voice" {
            let key = settings.model.voice_setting_key();
            set(&mut settings, key, value)?;
        }
    }
    let text = match (options.text.as_deref(), options.file.as_deref()) {
        (_, Some(file)) => std::fs::read_to_string(file)
            .map_err(|e| format!("couldn't read text file {file}: {e}"))?,
        (Some("-"), _) | (None, _) => {
            if std::io::stdin().is_terminal() {
                return Err("provide TEXT, --file PATH, or pipe stdin".into());
            }
            std::io::read_to_string(std::io::stdin())
                .map_err(|e| format!("couldn't read stdin: {e}"))?
        }
        (Some(text), _) => text.into(),
    };
    let pronunciations = pronunciations(&pronunciation_path(path)?)?;
    let root = crate::models::models_root().ok_or("couldn't resolve models directory")?;
    let chunks = crate::pipeline::prepare(
        &text,
        &crate::sanitize::SanitizerOptions::default(),
        &pronunciations,
    );
    if let Some(model) = crate::export::missing_models(&root, &chunks, &settings).first() {
        return Err(format!(
            "{} is not installed; use smart-voices download with the selected --model / --persian-voice",
            model.label()
        ));
    }
    let audio = crate::export::render(
        &text,
        &settings,
        &pronunciations,
        options.rate,
        |language| match language {
            crate::lang::Lang::English => crate::player::load_english_engine(&root, settings.model),
            crate::lang::Lang::Persian => {
                crate::player::load_persian_engine(&root, settings.persian_voice)
            }
        },
    )?;
    let bytes = crate::export::encode(&audio, format, options.float)?;
    if options.output == "-" {
        std::io::stdout()
            .lock()
            .write_all(&bytes)
            .map_err(|e| format!("couldn't write audio to stdout: {e}"))?;
    } else {
        // Encode completely before touching the destination, preserving existing files on encoder failure.
        std::fs::write(&options.output, bytes)
            .map_err(|e| format!("couldn't write audio to {}: {e}", options.output))?;
        eprintln!("Wrote {} (mono, {} Hz)", options.output, audio.sample_rate);
    }
    Ok(())
}
pub fn run(args: &[String]) -> Result<(), String> {
    if args == ["--version"] {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{HELP}");
        return Ok(());
    }
    if args.first().is_some_and(|a| a == "voices") {
        return match &args[1..] {
            [] => catalog(None),
            [flag, model] if flag == "--model" => catalog(Some(model)),
            _ => Err("usage: voices [--model fast|small|phonon|paradee|zonos]".into()),
        };
    }
    let path = crate::settings::settings_path().ok_or("couldn't resolve settings path")?;
    match args.first().map(String::as_str) {
        Some("serve") => crate::server::run(&args[1..], &path),
        Some("config") => config(&args[1..], &path),
        Some("synth" | "tts") => synth(&args[1..], &path),
        Some(command) if CONTROL_COMMANDS.contains(&command) => {
            control_command(command, &args[1..], &path)
        }
        _ => Err(HELP.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parser_checks_inputs_and_overrides() {
        let args = |s: &[&str]| s.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
        let parsed = parse_synth(&args(&[
            "-",
            "-o",
            "-",
            "--format",
            "wav",
            "--silence-scale",
            "0.5",
            "--set",
            "shortcuts.stop",
            "",
        ]))
        .unwrap();
        assert_eq!(parsed.text.as_deref(), Some("-"));
        assert_eq!(parsed.overrides[0], ("silence_scale".into(), "0.5".into()));
        let parsed = parse_synth(&args(&["hi", "--verbose", "--out", "a.wav"])).unwrap();
        assert_eq!(parsed.output, "a.wav");
        assert!(parsed.overrides.is_empty());
        for invalid in [
            &["--out"][..],
            &["hello", "--file", "a", "-o", "b.wav"],
            &["-o", "b.wav", "--sample-rate", "0"],
        ] {
            assert!(parse_synth(&args(invalid)).is_err());
        }
    }
    #[test]
    fn config_validates_without_mutating_on_failure() {
        let mut settings = Settings::default();
        for (key, value) in [
            ("speed", "0"),
            ("voice", "invalid"),
            ("phonon_voice", "invalid"),
            ("model", "invalid"),
            ("persian_voice", "invalid"),
            ("shortcuts.stop", "invalid"),
            ("silence_scale", "0"),
            ("resample_filter_zeros", "65"),
            ("unknown", "true"),
        ] {
            let before = settings.clone();
            assert!(set(&mut settings, key, value).is_err(), "{key}");
            assert_eq!(settings, before);
        }
        set(&mut settings, "speed", "1.25").unwrap();
        set(&mut settings, "shortcuts.stop", "").unwrap();
        let dir =
            std::env::temp_dir().join(format!("smart-voices-cli-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.toml");
        settings.save(&path).unwrap();
        assert_eq!(
            values(&load(&path).unwrap()).unwrap()["speed"],
            serde_json::json!(1.25)
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
