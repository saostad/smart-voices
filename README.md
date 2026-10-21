# Smart Voices

Smart Voices reads copied text aloud on Linux, Windows, and macOS. Copy text
anywhere and hear it read right away — Markdown from ChatGPT, Claude, or Grok
is spoken like a person would read it. It's a tiny tray app (about 20 MB)
that uses almost no CPU or memory when idle.

How it works: the app watches your clipboard, cleans up Markdown for speech
(tables read row by row, lists with ordinals, code announced instead of
spelled out, links shortened to site names, emoji removed), and reads it with
an on-device voice. English uses the Kokoro voice; Persian text automatically
switches to a Persian Piper voice. Everything runs on your computer — no text
ever leaves it.

## Downloads

Get the latest build from the
[Releases page](https://github.com/saostad/smart-voices/releases/latest).
Every push to `main` publishes a new release with:

- Windows: `smart-voices-windows-x86_64-setup.exe` — an installer that adds
  Smart Voices to the Start menu and to Apps & features for uninstalling. It
  isn't code-signed, so SmartScreen may warn: click More info → Run anyway.
- macOS (Apple Silicon): `smart-voices-macos-arm64.dmg` — open it and drag
  Smart Voices into Applications. The app is ad-hoc signed, so the first time
  right-click it and choose Open.
- Linux (Debian/Ubuntu): `smart-voices-linux-amd64.deb` — install with
  `sudo apt install ./smart-voices-linux-amd64.deb`.
- Linux (other distros): `smart-voices-linux-x86_64.tar.gz` — a plain binary
  plus `.desktop` file (see Linux notes below).

On first launch the app sits in your tray (menu bar on macOS). The first
reading downloads the voice model (see sizes below), then plays.

## First run: voice downloads

Models download automatically when first needed:

| Model | Size | When |
|---|---|---|
| Fast (default English voice) | ~350 MB | First English reading |
| Small (slower, low-memory English) | ~130 MB | Only if you pick it in Settings |
| Phonon (best-quality English) | ~103 MB | Only if you pick it in Settings |
| Phonon voices (37 English voices) | ~9.5 MB | First reading with a Phonon voice other than Default |
| Persian voice (Pocket, best quality) | ~230 MB | First Persian reading (English-only use never downloads it) |
| Persian voice (Piper, smaller) | ~67 MB | Only if you pick a Piper voice in Settings |

You can also pre-download from a terminal:
`smart-voices download`, `smart-voices download --model small`,
`smart-voices download --model phonon`, or `smart-voices download --persian`.

## The tray menu

By default every new copy is read aloud automatically, and a new copy stops
the current reading and reads the new text. Click the tray icon to open the
menu: a status line (`Ready`, `Reading 3 of 12`, `Paused`, …),
`Read Copied Text Automatically` (the main on/off switch), `Read Clipboard`,
`Pause`/`Resume`, `Stop`, `Previous`/`Next Sentence`, `Show Reading Panel`,
the `Settings` submenu, and `Quit`.

While reading, a small panel shows the text with the current word
highlighted; click a paragraph to jump there, or use its Previous / Pause /
Next buttons. Close it to hide it for that reading.

The `Settings` submenu holds everything customizable:

- `Voice Model` — Fast, Small or Phonon; switching reloads the voice and
  changes which voices `English Voice` lists.
- `English Voice` — the selected model's voices: 28 Kokoro voices grouped by
  accent and gender for Fast/Small, or Default plus 37 catalog voices in
  American, British and Characters groups for Phonon.
- `Persian Voice` — `Pocket v2 (best quality — recommended)` is the default
  and sounds the most natural; `Pocket v1` below it. The Piper voices (Amir,
  Gyro, Reza Ibrahim, Ganji, Ganji Adabi) are smaller downloads and start
  faster, with lower quality.
- `Reading Speed` — 0.75× to 2×.
- `Launch at Login` — start with your computer (reflects the real system
  state).
- `Watch the Clipboard` — turn clipboard watching off entirely.
- `Show Reading Panel While Reading` — show the panel automatically while
  reading.
- `Animate Icon While Reading` — animate the tray icon while reading.
- `Free Memory When Idle` — free the voice memory after 1–60 minutes, or
  `Never`.
- `Shortcuts` — shows the current bindings; change them in `settings.toml`
  and restart.
- `Edit Pronunciations…` / `Open Settings File…` — teach the app words
  (SQL → "sequel"); edits apply on the next reading.

## Shortcuts

Global shortcuts work everywhere, even when another app is focused:

| Action | macOS | Linux / Windows |
|---|---|---|
| Read / Pause | ⌃⌥Space | Ctrl+Alt+Space |
| Stop | ⌃⌥. | Ctrl+Alt+. |
| Next Sentence | ⌃⌥→ | Ctrl+Alt+→ |
| Previous Sentence | ⌃⌥← | Ctrl+Alt+← |

Set a binding to `""` in `settings.toml` to disable it.

## Where things live

| | macOS | Linux | Windows |
|---|---|---|---|
| Settings | `~/Library/Application Support/smart-voices/settings.toml` | `~/.config/smart-voices/settings.toml` | `%APPDATA%\smart-voices\settings.toml` |
| Pronunciations | same folder, `pronunciations.toml` | same folder | same folder |
| Voice models | `~/Library/Application Support/smart-voices/models/` | `~/.local/share/smart-voices/models/` | `%APPDATA%\smart-voices\models\` |

Override the settings file with the `SMART_VOICES_CONFIG` environment
variable (full file path) and the models folder with `SMART_VOICES_MODELS`.

## Linux notes

- On GNOME, install the "AppIndicator and KStatusNotifierItem Support"
  extension to see the tray icon. The `.deb` installs everything for you; with
  the `.tar.gz`, copy `smart-voices` to `~/.local/bin` and
  the `.desktop` file from the download to `~/.local/share/applications` (and
  to `~/.config/autostart` to start at login).
- Clipboard watching needs X11 or XWayland. On pure Wayland without XWayland,
  use the tray icon / shortcuts to read instead.
- Global shortcuts use the XDG GlobalShortcuts portal on Wayland sessions
  (your desktop may ask you to confirm them once; needs GNOME 48+, KDE 6, or
  similar) and a classic key grab on X11.

## Building from source

You need a Rust toolchain (stable). From the repository root:

```sh
cargo build --release
```

The binary lands at `target/release/smart-voices`. Keep the shared
libraries next to it (`libsherpa-onnx-c-api.*` and `libonnxruntime.*` — the
build copies them there): the app loads its voice runtime from them. On
Linux, install the audio headers first:
`sudo apt-get install libasound2-dev pkg-config`. On macOS,
`scripts/bundle-macos.sh` wraps the binary in `Smart Voices.app`.

## Command line

Running `smart-voices` with no arguments starts the tray app. It also has
headless commands (handy for scripts and testing):

- `smart-voices speak "<text>"` — read text aloud (`--voice`, `--speed`,
  `--model fast|small|phonon`, `--persian-voice pocket|amir|gyro|reza_ibrahim|ganji|ganji_adabi`, `--verbose`).
  With `--model phonon`, `--voice` takes a catalog voice name (e.g. `Zoey`) or `default`.
- `smart-voices clean [<file>]` — print how text would be cleaned for speech,
  one chunk per line (reads stdin without a file).
- `smart-voices download` — fetch voice models up front (`--model`,
  `--persian`).
- `smart-voices watch` — print each copy and, with `--auto-read`, read it
  aloud (Ctrl+C quits).

## Performance

Measured on an M1 Pro Mac: idle tray uses about 11–50 MB and 0% CPU (the
voice unloads after 15 idle minutes); synthesis runs at about 0.2×
real-time (5× faster than playback). From copy to first sound takes about
0.3–0.5 seconds while the voice is loaded, and about 1.5 seconds right after
it was unloaded.
