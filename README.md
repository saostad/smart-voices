# Smart Voices

Smart Voices is a native macOS menu-bar app that reads text aloud with local,
on-device voices. Copy Markdown from ChatGPT, Claude, or Grok, press play, and
hear it read like a person would — headings, lists, tables, and code are spoken
naturally instead of reading the markup. Everything runs 100% on your Mac; no
text ever leaves your computer.

## Features

- **Clipboard reading** — copy text, click the menu-bar icon (or press play) to
  hear it. A play badge lights up when new text is copied; password managers
  and ignored apps never trigger it.
- **Markdown-aware speech** — Grok-style reading: tables read row by row, lists
  with ordinals ("Second, …"), code announced instead of spelled out, links
  shortened to site names, emoji and citations removed.
- **Conversational mode (optional)** — retells text in a natural spoken style
  with Apple's on-device model before reading. Requires macOS 26 with Apple
  Intelligence.
- **Local Kokoro voice** — the natural Kokoro-82M voice runs on-device
  (~120 MB download on first launch), plus all built-in macOS voices.
- **Language-aware** — non-English text (Persian, German, …) automatically
  switches to a matching installed macOS voice; Kokoro speaks English.
- **Reading panel** — a floating window that highlights the current sentence
  and word, with click-to-jump, transport controls, and speed.
- **Pronunciation dictionary** — teach Smart Voices how to say names, acronyms,
  and jargon (SQL → "sequel", JSON → "jason", …).
- **More ways to read** — "Read Aloud with Smart Voices" Service for selected
  text in any app, Read File… (`.md`, `.txt`, `.rtf`, `.pdf`), drag and drop
  onto the reading panel, and auto-read on copy.
- **Extras** — recent history with replay, export to `.m4a`, media keys and
  Control Center (Now Playing), sleep timer, global shortcuts, launch at login.

## Requirements

- Apple Silicon Mac, macOS 15 or later.
- Conversational mode additionally needs macOS 26 with Apple Intelligence
  enabled.
- Xcode toolchain to build (`swift build`).

## Install and run

Download `SmartVoices-1.0.0.dmg` from the
[releases page](https://github.com/saostad/smart-voices/releases), open it, and
drag Smart Voices to Applications. (The app is ad-hoc signed, so on first
launch right-click it and choose Open.)

Or build from source:

```sh
scripts/build-app.sh && open "build/Smart Voices.app"
```

Every push to `main` automatically builds and publishes a new DMG release.

Click the menu-bar icon to read the clipboard. Right-click (or Control-click)
for the menu: Watch the Clipboard, Read New Copies Automatically,
Read Clipboard (⌘R), Read File… (⌘O), Recent, Pause/Resume, Stop,
Previous/Next Sentence, Show Reading Panel (⌘P), Export Clipboard as Audio…,
Speed, Sleep Timer, Voice, Engine, Settings… (⌘,), Quit (⌘Q).

The two clipboard checkboxes stay in sync with Settings → General. Uncheck
Watch the Clipboard to stop watching new copies, or Read New Copies
Automatically to keep watching without starting playback automatically.

Default global shortcuts (change them in Settings → Shortcuts):

- Play/Pause: `Ctrl+Opt+Space`
- Stop: `Ctrl+Opt+.`
- Next/Previous sentence: `Ctrl+Opt+→` / `Ctrl+Opt+←`

## Read selected text from any app

The first time, enable the Service in System Settings → Keyboard → Keyboard
Shortcuts → Services → Text → "Read Aloud with Smart Voices", where you can
also assign a shortcut. Then select text anywhere and use the Services menu.

## Settings

- **General** — launch at login, clipboard watching, auto-read, language
  switching, reading panel, history, ignored apps.
- **Voice** — engine (Kokoro / macOS Voice), voice, speed.
- **Reading** — how code blocks, tables, and links are spoken, conversational
  mode, and a live preview of exactly what will be said.
- **Pronunciation** — the Written/Says dictionary with per-row previews.
- **Shortcuts** — global keyboard shortcuts.
- **About** — version, voice-model folder, credits.

## For developers

```sh
swift build
swift test
swift run svtext < some.md   # print sanitized speech chunks, one per line
```

Headless checks (no UI):

```sh
swift run SmartVoices --say "Hello there."
swift run SmartVoices --say-file sample.md --engine system --conversational
swift run SmartVoices --export out.m4a --say-file sample.md
```

## Credits

Voice: Kokoro-82M (Apache 2.0). Engine: FluidAudio. Thanks to the open-source
projects that make on-device speech possible.
