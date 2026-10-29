Smart Voices for Linux
=====================

Install
-------
1. Copy `smart-voices` and the `libonnxruntime.so*` files next to it into the same folder,
   e.g. `~/.local/bin` (make sure it is on your PATH). Keep them together:
   the app loads its voice runtime from those libraries.
2. Copy `smart-voices.desktop` to `~/.local/share/applications`.
3. To start Smart Voices at login, also copy `smart-voices.desktop`
   to `~/.config/autostart`.

Notes
-----
- On GNOME, install the "AppIndicator and KStatusNotifierItem Support"
  extension to see the tray icon.
- Clipboard watching needs X11 or XWayland.
- The first reading downloads the voice model (~350 MB for English,
  ~230 MB more the first time you read Persian).
