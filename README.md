# Smart Voices

Smart Voices is a native macOS menu-bar app that reads copied Markdown aloud with a local TTS voice, speaking headings, lists, tables, and code like a person would instead of reading markup.

```
swift build
swift test
swift run svtext < some.md
```

## Run the app

```
scripts/build-app.sh && open "build/Smart Voices.app"
```

Click the menu-bar icon to read the clipboard aloud. Right-click (or Control-click) for the menu: Read Clipboard (⌘R), Pause/Resume, Stop, Previous/Next Sentence, Speed, Quit (⌘Q). Headless check: `swift run SmartVoices --say "Hello there."`.

To read selected text from any app, enable the service the first time in System Settings → Keyboard → Keyboard Shortcuts → Services → Text → "Read Aloud with Smart Voices", where you can also assign a shortcut.
