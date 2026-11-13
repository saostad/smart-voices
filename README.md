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

The tray status and reading panel show which voice is downloading, with a
percentage and downloaded/total MB when the server provides the size. Without
a size, they show the MB received. A desktop notification announces each model
download once; reading starts automatically when it finishes. New copies
replace the text waiting to be read. Stop cancels the pending reading while
letting the download finish for next time.

| Model | Size | When |
|---|---|---|
| Fast (default English voice) | ~350 MB | First English reading |
| Small (slower, low-memory English) | ~130 MB | Only if you pick it in Settings |
| English pronunciation data (Kokoro/Paradee) | ~18 MB | First Kokoro or Paradee English reading (including upgrades) |
| Paradee (tiny, fastest English) | ~9 MB | Only if you pick it in Settings |
| English espeak-ng data (Paradee) | ~7 MB | First Paradee English reading |
| Phonon (best-quality English) | ~103 MB | Only if you pick it in Settings |
| Phonon voices (37 English voices) | ~9.5 MB | First reading with a Phonon voice other than Default |
| Zonos (studio-quality English, for fast computers) | ~1.1 GB | Only if you pick it in Settings |
| Persian voice (Pocket, best quality) | ~230 MB | First Persian reading (English-only use never downloads it) |
| Persian voice (Piper, smaller) | ~67 MB | Only if you pick a Piper voice in Settings |

You can also pre-download from a terminal:
`smart-voices download`, `smart-voices download --model small`,
`smart-voices download --model paradee`, `smart-voices download --model phonon`,
`smart-voices download --model zonos`, or `smart-voices download --persian`.
These commands print download progress in the terminal too.

## The tray menu

By default every new copy is read aloud automatically, and a new copy stops
the current reading and reads the new text. Click the tray icon to open the
menu: a status line (`Ready`, `Downloading English voice (Fast)… 42%`,
`Loading voice…`, `Preparing…`, `Reading 3 of 12`, `Paused`, …),
`Read Copied Text Automatically` (the main on/off switch), `Read Clipboard`,
`Pause`/`Resume`, `Stop`, `Previous`/`Next Sentence`, `Show Reading Panel`,
the `Settings` submenu, and `Quit`.

While reading, a small panel shows the text with the current word
highlighted; click a paragraph to jump there, or use its Previous / Pause /
Next buttons. Close it to hide it for that reading.
The panel also opens during downloads and voice loading if automatic display
is enabled, and shows download progress before audio starts. Voice loads that
take more than three seconds trigger one desktop notification. Download,
voice-loading, and playback failures appear in the tray status, the panel,
and a desktop notification. Notifications follow your desktop's notification
permissions and Do Not Disturb settings.

The `Settings` submenu holds everything customizable:

- `Voice Model` — Fast, Small, Paradee, Phonon or Zonos; switching reloads
  the voice and changes which voices `English Voice` lists.
- `English Voice` — the selected model's voices: 28 Kokoro voices grouped by
  accent and gender for Fast/Small, 4 distilled Kokoro voices for Paradee,
  Default plus 37 catalog voices in American, British and Characters groups
  for Phonon, or 10 voices in American, British · Irish and Characters
  groups for Zonos.
- `Persian Voice` — `Pocket v2 (best quality — recommended)` is the default
  and sounds the most natural; `Pocket v1` below it. The Piper voices (Amir,
  Gyro, Reza Ibrahim, Ganji, Ganji Adabi) are smaller downloads and start
  faster, with lower quality.
  English words inside Persian text are read with Persian pronunciation.
  Unknown words are approximated, and a notice says so.
  To fix a word, open `Edit Pronunciations…` and add it with its Persian spelling.
- `Reading Speed` — 0.75× to 2×.
- `Launch at Login` — start with your computer (reflects the real system
  state).
- `Watch the Clipboard` — turn clipboard watching off entirely.
- `Show Reading Panel While Reading` — show the panel automatically while
  reading.
- `Animate Icon While Reading` — animate the tray icon while reading.
- `Read Numbers and Symbols Naturally` — with English voices, say numbers,
  dates, money, codes, units and symbols the way a person would.
- `Read Dates Like 09/10 As` — month first (the default) or day first, for
  dates that could be either. A date elsewhere in the text that can only be
  one way, like 25/10, settles it for that text.
- `Fix Hard Names and Foreign Words` — off by default. With English voices,
  respell about 25,000 hard names and foreign words. It helps names a voice
  gets badly wrong and can hurt ones it already says well.
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

You need a Rust toolchain (stable), CMake, a C/C++ compiler, and libclang (for espeak-ng bindings). From the repository root:

```sh
scripts/fetch-onnxruntime.sh
cargo build --release
```

The binary lands at `target/release/smart-voices`. Keep `libonnxruntime.so*`
(Linux), `libonnxruntime*.dylib` (macOS), or `onnxruntime.dll` (Windows)
beside it. The fetch script verifies pinned archives from sherpa-onnx
v1.13.8 and copies only ONNX Runtime 1.28.2; no sherpa libraries are used.
espeak-ng 1.52 is compiled from source and linked statically through
`espeak-rs-sys`; on Windows it uses the static CRT. On
Linux, install the audio headers first:
`sudo apt-get install libasound2-dev pkg-config cmake libclang-dev`. On macOS,
`scripts/bundle-macos.sh` wraps the binary in `Smart Voices.app`.

## Command line

Running `smart-voices` with no arguments starts the tray app. Everything in the
tray menu is also available from the command line and the [HTTP API](#http-api):

| Tray menu | Command | HTTP API |
|---|---|---|
| Read Clipboard | `read-clipboard` | `POST /v1/playback {"clipboard":true}` |
| A new copy is read | `play TEXT`, `play -`, `play --file PATH` | `POST /v1/playback {"text":"…"}` |
| Pause / Resume | `pause`, `resume`, `toggle` | `POST /v1/playback/pause`, `/resume`, `/toggle` |
| Stop | `stop` | `POST /v1/playback/stop` |
| Previous / Next Sentence | `previous`, `next` | `POST /v1/playback/previous`, `/next` |
| Click a paragraph in the panel | `jump N` (from 1) | `POST /v1/playback/jump {"index":N}` (from 0) |
| Status line, panel text | `status [--json]` | `GET /v1/playback` |
| Show Reading Panel | `show-panel` | `POST /v1/panel` (tray only) |
| Quit | `quit` | `POST /v1/quit` |
| Every Settings entry | `config get`, `config set`, `config reset` | `GET`, `PATCH /v1/settings` |
| Launch at Login | `config set autostart true` | `GET`, `PUT /v1/autostart` |
| Edit Pronunciations | `config get`, `set`, `reset pronunciations` | `GET`, `PUT`, `DELETE /v1/pronunciations` |
| Open Settings File, Change Shortcuts | `config path`, `config set shortcuts.stop …` | `GET`, `PATCH /v1/settings` |
| Voice download progress | `download` | `POST`, `GET /v1/downloads` |
| Voice lists | `voices` | `GET /v1/capabilities` |

The commands in the first rows act on the running tray app, or on
`smart-voices serve` if there's no tray, so `smart-voices pause` pauses whatever
is reading. `config set` applies right away on a running app, except `server.*`
and `shortcuts.*`, which need a restart. Set `SMART_VOICES_URL` and
`SMART_VOICES_TOKEN` to control a service on another machine.

These commands need nothing running (handy for scripts and testing):

- `smart-voices speak "<text>"` — read text aloud (`--voice`, `--speed`,
  `--model fast|small|phonon|paradee|zonos`, `--persian-voice pocket|amir|gyro|reza_ibrahim|ganji|ganji_adabi`, `--verbose`).
  With `--model phonon`, `--voice` takes a catalog voice name (e.g. `Zoey`) or `default`;
  with `--model paradee`, it takes one of its four voice names; with
  `--model zonos`, one of its voice names (e.g. `Savannah`).
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

Kokoro and Paradee also download English pronunciation data (~18 MB, once):
the original misaki US/British dictionaries (Apache-2.0) and misaki-rs POS
tagger (MIT). Only the voice's regional dictionaries are loaded. Unknown words
and numeric words use the same statically linked espeak-ng as Persian Piper.
Paradee (Apache-2.0, distilled from Kokoro) additionally downloads the
standalone espeak-ng data bundle (~7 MB) because its own files contain no
`espeak-ng-data` directory.

### Zonos (studio quality, for fast computers)

Zonos is [ZONOS3-Mini](https://huggingface.co/Zyphra/ZONOS3-Mini) by Zyphra
(Apache-2.0): the most natural English voice here, with a ~1.1 GB download
and about 1.4 GB of memory while loaded. It runs in `zonos3-server`, Zyphra's
native runtime, which ships next to the app (about 6 MB; built from a pinned
commit by `scripts/build-zonos3.sh`) and only starts when Zonos is the
selected model. The app starts it on a free localhost port, and it stops when
the voice unloads or the app quits. On Linux and Windows it runs on the CPU
and needs AVX2 (most CPUs from 2013 on); on Apple Silicon it uses the GPU.
Persian text still uses the Persian voice.

On a 2015 quad-core laptop CPU (i7-6700HQ) it synthesizes a paragraph in
about 0.6–0.7× its playback time, and the runtime takes about 2 seconds to
start; newer machines are much faster.

`settings.toml` takes a `[zonos]` table:

| Key | Default | Meaning |
|---|---|---|
| `startup_timeout_secs` | 120 | Seconds to wait for the runtime to load the model (1–3600) |
| `request_timeout_secs` | 600 | Seconds to wait for one sentence's audio (1–3600) |
| `use_gpu` | `true` on macOS, else `false` | Run on the GPU (Metal); CPU-only builds ignore it |
| `server_path` | empty | A `zonos3-server` to use instead of the bundled one, e.g. your own CUDA or Vulkan build |
| `batch_slots` | 1 | Generation slots the runtime reserves (1–64); one reader needs one, more slow startup and reading |
| `decoder_lanes` | 1 | Parallel waveform decoders (1–16); more compete with generation for the CPU |
| `rate_bytes_per_sec` | 15 | Speaking rate asked of the model for long text, in UTF-8 bytes per second (5–30) |
| `chunk_overhead_secs` | 0.25 | Extra seconds each sentence gets, so one-word sentences aren't rushed (0–2) |
| `laci` | `true` | Zyphra's LACI guard, which redoes skipped text and invented audio |

Without a speaking rate the model guesses how long a short sentence lasts and
often keeps talking after it ("Sure!" came out as two seconds of made-up
words), so the app always asks for one. Reading Speed is applied after
synthesis, like Phonon: asking the model to speak faster than its natural
rate makes it drop words.

`SMART_VOICES_ZONOS3_SERVER` also points at another runtime when
`server_path` is empty. The voice is `zonos_voice` (default `savannah`).

`settings.toml` takes a `[pocket]` table for the Persian Pocket voices:

| Key | Default | Meaning |
|---|---|---|
| `max_tokens` | 50 | Longest chunk sent to the model, in tokens (8–50); higher means fewer, longer pieces |
| `min_tokens` | 8 | Shortest chunk the planner aims for, in tokens (1–`max_tokens`); higher means fewer tiny pieces |
| `temperature` | 0.3 | Sampling randomness (0–1.5); higher means more varied, less predictable speech |
| `decode_steps` | 0 | Sampler decode steps (0–4); higher may improve quality at more cost, and 0 uses the model's own value |
| `eos_threshold` | -2 | End-of-speech logit threshold (-10–10); higher means the model stops sooner |
| `frames_after_eos` | 0 | Frames generated after the end-of-speech signal (0–10); higher means a longer tail |
| `g2p_window_chars` | 120 | Characters per pronunciation window for long sentences (40–1000); higher means fewer, longer windows |

`smart-voices synth … --set pocket.max_tokens 30` tries a value for one run without saving it.

`settings.toml` accepts `silence_scale = 0.2` (0.01–10, also available as
Pause Length in the tray), `pause_min_seconds = 0.2` (positive finite seconds),
and `pause_amplitude = 0.01` (0–1). The latter two control pause detection.

Piper inference scales are read from the tarball’s `<voice>.onnx.json` file
(the Persian ONNX files do not contain those metadata fields).

Playback resampling also accepts `resample_cutoff_ratio = 0.99` (0–1,
exclusive of 0) and `resample_filter_zeros = 6` (1–64) in settings.toml.
These retain the old resampler’s cutoff and window defaults.

### Command-line audio export and settings

Generate audio without a tray, desktop session, or audio output device:

```sh
smart-voices synth "Hello world." -o hello.wav
smart-voices synth --file article.md -o article.flac --speed 1.25
printf 'Hello world.' | smart-voices synth -o - --format mp3 > hello.mp3
smart-voices speak "Hello world." --out hello.ogg --model phonon
smart-voices synth "Hello." -o float.wav --float --sample-rate 48000
smart-voices synth "Hello." -o hello.wav --silence-scale 0.5 --set resample_filter_zeros 12
```

Input is one quoted text argument, `--file PATH`, or stdin (text `-`, or
omitted when piped). Markdown cleanup, pronunciation rules, language detection,
Persian voice selection and chunk pauses match playback. Use `--` before text
starting with `--`. Existing `speak TEXT` still plays through speakers.

WAV and raw PCM (`.pcm` / `.raw`) are built in: mono, signed 16-bit
little-endian by default, or IEEE float32 with `--float`. FLAC, MP3 and Ogg
Opus (`.ogg` / `.opus`) require **ffmpeg on PATH**, with the `flac`,
`libmp3lame` and `libopus` encoders respectively. No substitute format is
written if an encoder is missing. These external encoders add no bundled
dependencies or licensing changes to the app binary; your ffmpeg distribution
has its own license. Its default compression settings apply. Output extension
selects the format; `--format` must agree if supplied. Stdout and extensionless
paths require `--format`. Progress/errors go to stderr; stdout contains audio
only for `--out -`. Output uses the first chunk's sample rate by default;
mixed-rate chunks are resampled with the configured filter.

Export requires installed models and never downloads them implicitly. Discover
models/voices with `smart-voices voices` or `voices --model phonon`; install
with `smart-voices download --model phonon` (or another model), and
`download --persian-voice pocket-v2` for Persian. All settings use saved defaults.
Override any Settings key for one export with `--key-name VALUE` or
`--set dotted.key VALUE`. For example `--phonon-voice default`,
`--persian-voice amir`, `--pause-min-seconds 0.1`, or
`--set shortcuts.stop ""`. Desktop-only settings have no effect on file export.

```sh
smart-voices config list                 # All values and voice/model catalogs
smart-voices config list --json          # Machine-readable values
smart-voices config get shortcuts.stop
smart-voices config set speed 1.25
smart-voices config set shortcuts.stop 'ctrl+alt+Q'
smart-voices config set autostart true   # OS Launch at Login setting
smart-voices config get pronunciations
smart-voices config set pronunciations '[{"written":"SQL","spoken":"sequel","case_sensitive":false,"whole_word":true}]'
smart-voices config get pronunciations.path
smart-voices config reset speed
smart-voices config reset                # Reset settings.toml, keeping dictionary/login item
smart-voices config path
```

Settings changes are validated by the same service used when loading/saving UI
settings. Speed must be 0.5–2; voice/model values must be catalog IDs. Shortcut
values use the app's existing shortcut parser; an empty string disables one.
Pronunciation set replaces the full dictionary; reset restores built-in rules.
Autostart reset disables Launch at Login. Restart a running tray app after
editing settings: it holds settings in memory and may overwrite external edits.
Pronunciations reload at the next reading; autostart updates the OS immediately.
`SMART_VOICES_CONFIG` selects an alternate settings file (the pronunciation file
is adjacent); `SMART_VOICES_MODELS` selects the models directory.
Run `smart-voices --help` for the complete CLI usage.

## HTTP API

Run a service with warm, lazy-loaded engines. `/v1/speak` needs no audio device,
and models are downloaded only when you ask with `POST /v1/downloads`:

```sh
smart-voices download --model fast
smart-voices download --persian-voice pocket-v2
smart-voices serve
curl http://127.0.0.1:7341/v1/health
curl http://127.0.0.1:7341/v1/capabilities
curl http://127.0.0.1:7341/v1/speak -H 'Content-Type: application/json' \
  -d '{"text":"Hello world.","format":"ogg"}' -o hello.ogg
curl http://127.0.0.1:7341/v1/speak -H 'Content-Type: application/json' \
  -d '{"text":"سلام دنیا.","language":"fa","format":"wav"}' -o persian.wav
curl http://127.0.0.1:7341/v1/speak -H 'Content-Type: application/json' \
  -d '{"text":"Hello.","timings":true}'
```

Authentication is optional on **every** listen address. Prefer localhost or a
Tailscale address. When no token is configured, anything that can reach the
port can use the service. For a Docker BFF, configure host access explicitly
(e.g. host.docker.internal with a host-gateway mapping on Linux), bind a
reachable interface and protect it with a firewall and/or token. Bearer tokens
over plain HTTP are not encrypted; use Tailscale or a trusted TLS proxy.

```sh
umask 077
openssl rand -hex 32 > "$HOME/.config/smart-voices/http-token"
smart-voices serve --listen 127.0.0.1:7341 \
  --token-file "$HOME/.config/smart-voices/http-token"
# Health is always public, even with a token:
curl http://127.0.0.1:7341/v1/health
curl http://127.0.0.1:7341/v1/capabilities \
  -H "Authorization: Bearer $(cat "$HOME/.config/smart-voices/http-token")"
curl http://127.0.0.1:7341/v1/speak \
  -H "Authorization: Bearer $(cat "$HOME/.config/smart-voices/http-token")" \
  -H 'Content-Type: application/json' \
  -d '{"text":"Hello.","format":"wav"}' -o hello.wav
```

The token file is trimmed and read once at startup; an empty file is an error.
Restart after changing it. Tokens, request text and audio are never logged.
Startup logs address/auth/limits; each request logs method/path/status/time and
character count. SIGINT/SIGTERM stop admission and drain accepted jobs.

All flags override the matching persisted `server.*` key for this run only.
For example, `smart-voices config set server.max_chars 20000`.
The settings table and defaults are:

| Flag | Settings key | Default | Validation |
|---|---|---|---|
| --listen | server.listen | 127.0.0.1:7341 | IP address:port (IPv6 in brackets) |
| --token-file | server.token_file | "" (auth off) | readable nonempty file when set |
| --max-concurrency | server.max_concurrency | 1 | positive integer |
| --read-timeout-secs | server.read_timeout_secs | 10 | positive seconds, at most u32::MAX; total headers/body deadline |
| --max-header-bytes | server.max_header_bytes | 16384 | positive integer |
| --accept-poll-ms | server.accept_poll_ms | 10 | positive milliseconds, at most u32::MAX; accept/shutdown polling interval |
| --linger-drain-bytes | server.linger_drain_bytes | 1048576 | positive integer |
| --linger-secs | server.linger_secs | 2 | positive seconds, at most u32::MAX |
| --max-lingering-rejections | server.max_lingering_rejections | 4 | positive integer |
| --max-handlers | server.max_handlers | 16 | positive integer; simultaneous request handlers |
| --max-queue | server.max_queue | 4 | nonnegative integer |
| --max-chars | server.max_chars | 20000 | positive Unicode character count |
| --max-body-bytes | server.max_body_bytes | 1048576 (1 MiB) | positive integer |
| --idle-unload-secs | server.idle_unload_secs | 1200 | nonnegative integer; 0 keeps engines warm |
| (tray only) | server.tray_listen | 127.0.0.1:0 | IP address:port; port 0 picks a free port |
| (tray only) | server.tray_accept_poll_ms | 250 | positive milliseconds, at most u32::MAX |

`GET /v1/health` returns contract/version/ready/missing/auth/modelsLoaded.
It is always unauthenticated. `ready` is true only when the configured English
model (including support data) and Persian voice are installed; `missing` lists
the missing model labels (empty when ready). This uses filesystem checks only.
Request body reads run in separate handlers bounded by `max_handlers`; overflow
returns 429 without reading its body. Headers and body share a total read deadline
(`read_timeout_secs`); header size is bounded by `max_header_bytes`.
Connections close after one response; send `Content-Length` for request bodies.
Chunked transfer encoding is rejected with 400. Shutdown drains admitted synthesis
jobs, not uploads.
`GET /v1/capabilities` lists English and Persian models, installation state,
voices, available encoders, languages and limits. WAV is built in; Ogg Opus
and MP3 require ffmpeg with libopus/libmp3lame.

`POST /v1/speak` accepts JSON only; unknown fields are rejected:

| Field | Meaning |
|---|---|
| text | Required, nonempty after cleanup |
| language | auto (default), en, fa; auto switches per chunk as the CLI does |
| voice | Persian voice id overrides persian_voice for this request; otherwise an English voice id for the configured model. See /v1/capabilities for valid ids; never persisted |
| speed | Optional 0.5–2 multiplier |
| format | ogg (default, Opus), wav, mp3 |
| input | markdown (default sanitizer), plain (whitespace cleanup only) |
| maxChars | Optional positive limit no greater than the service limit |
| timings | false (default) returns audio; true returns JSON with base64 audio |
| model | English model for this request: fast, small, phonon, paradee or zonos; never persisted |
| normalizeText | Read numbers and symbols naturally; overrides the setting for this request |
| respellNames | Respell hard names and foreign words; overrides the setting for this request |
| dateOrder | month_first or day_first; overrides the setting for this request |
| silenceScale | Pause length multiplier, 0.01–10; overrides the setting for this request |

Explicit en/fa reject chunks detected in the other language with code 7;
use auto for mixed text. Per-request settings never modify settings.toml.
Audio responses include Content-Type, X-Contract, X-Duration-Sec, X-Voice,
X-Model, X-Language (en/fa/mixed), X-Sample-Rate. Mixed metadata joins voice
and model IDs with commas. Timings JSON includes contract, version, mimeType,
durationSec, language, voice, model, sampleRate, audioBase64, words and
wordsAvailable. The engines have no real timestamps: words is [] and
wordsAvailable is false, rather than estimated timestamps.

Every error uses
`{"contract":1,"error":{"code":2,"kind":"invalid_request","message":"..."}}`.
Clients should branch on code, which is null for transport/auth errors.

| HTTP | code | Meaning |
|---|---|---|
| 400 | 2 | Bad JSON, unknown field, invalid enum/range |
| 401 | null | Missing/wrong bearer token; WWW-Authenticate: Bearer |
| 404 / 405 | null | Unknown route / wrong method |
| 413 | 5 | Body or effective character limit exceeded |
| 415 | 4 | Non-JSON content type or requested encoder unavailable |
| 422 | 4 | Empty text after cleanup |
| 422 | 7 | Requested language cannot serve detected text |
| 429 | null | Queue full; Retry-After: 1 (seconds) |
| 500 | 6 | Synthesis or encoding failed |
| 503 | 3 | Required model missing; message gives the exact download command |

### Control routes

`smart-voices serve` and the tray app both serve these, so the CLI and other
clients can do what the tray menu does. The tray listens on `server.tray_listen`
(default: a free local port) and always requires a token: it generates one
unless `server.token_file` is set, and publishes its address and token in
`tray.control.json` next to the settings file, readable only by you. `serve`
publishes `serve.control.json` the same way.

Control routes are stricter than `/v1/speak`, because they change settings and
play sound: any request with an `Origin` header gets 403, so a web page can't
reach them, and without a token only this machine may call them (403 otherwise).
`POST`, `PUT`, `PATCH` and `DELETE` need `Content-Type: application/json`.

| Route | Body | Result |
|---|---|---|
| `GET /v1/playback` | | Playback status |
| `POST /v1/playback` | `{"text":"…"}` or `{"clipboard":true}` | Starts reading aloud; status |
| `POST /v1/playback/pause`, `resume`, `toggle`, `stop`, `next`, `previous` | `{}` | Status |
| `POST /v1/playback/jump` | `{"index":N}` | Goes to chunk N (from 0); status |
| `GET /v1/settings` | | `settings` (flat, dotted keys), `paths`, `restartRequired` |
| `PATCH /v1/settings` | `{"speed":1.25,"date_order":"day_first"}` | Saves and applies all or none |
| `GET`, `PUT`, `DELETE /v1/pronunciations` | PUT: `{"entries":[{"written":"GUI","spoken":"gooey"}]}` | `entries`; DELETE resets to defaults |
| `GET`, `PUT /v1/autostart` | PUT: `{"enabled":true}` | `enabled` |
| `POST /v1/downloads` | `{"model":"phonon"}` and/or `{"persianVoice":"amir"}` | 202; downloads in the background |
| `GET /v1/downloads` | | `active`, and each job's `state`, `doneBytes`, `totalBytes`, `error` |
| `POST /v1/panel` | `{}` | Opens the reading panel; 409 on `serve`, which has no window |
| `POST /v1/quit` | `{}` | Stops the host |

Playback status is `{contract, host, state, status, index, count, text, chunks,
error}`, with `state` one of `idle`, `loading`, `playing` or `paused`. On `serve`,
playback opens the audio device on the first reading, and a missing voice returns
503 instead of downloading; the tray downloads it, as a copied text does.

If `serve` and the tray both run, each reads on its own: the CLI talks to the
tray first.

For a systemd user service, copy
`packaging/linux/smart-voices-serve.service` to
`~/.config/systemd/user/`, adjust ExecStart to your binary path, then run
`systemctl --user daemon-reload` and
`systemctl --user enable --now smart-voices-serve.service`.
Set server defaults with `config set` before starting it.

Early HTTP rejections send the full response and FIN, then discard incoming bytes
until EOF, `server.linger_drain_bytes`, or `server.linger_secs`. Overflow
rejections run outside the accept loop, bounded by `server.max_lingering_rejections`.
If both handler and rejection capacity are exhausted, new sockets are closed
without a response and a warning is logged. Fully consumed requests close immediately.

## Credits

The English text normalizer (`src/normalize.rs`) and name lexicon
(`assets/respell.tsv`) are ported from
[Underdog Woof](https://huggingface.co/ConwayResearch/Underdog-Woof), under the
Underdog Non-Commercial License 1.0: free for non-commercial use with credit.
Any commercial use of this app would need a license from Underdog or both
features removed.
