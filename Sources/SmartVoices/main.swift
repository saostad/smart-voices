import AppKit
import Foundation
import Observation
import os
import SmartVoicesCore
import SmartVoicesKokoro

private let appLogger = Logger(subsystem: "dev.smartvoices", category: "app")

func writeStderr(_ message: String) {
    if let data = (message + "\n").data(using: .utf8) {
        FileHandle.standardError.write(data)
    }
}

func msBetween(_ start: ContinuousClock.Instant, _ end: ContinuousClock.Instant) -> Int {
    let duration = start.duration(to: end)
    return Int(Double(duration.components.seconds) * 1000 + Double(duration.components.attoseconds) / 1_000_000_000_000_000)
}

func makeHeadlessEngine(engineID: String) -> any SpeechEngine {
    if engineID == "kokoro" {
        return KokoroSpeechEngine()
    }
    return SystemSpeechEngine()
}

func runHeadlessAsync(text: String, engine: any SpeechEngine) async -> Int32 {
    let sanitizer = SpeechSanitizer()
    let chunker = SpeechChunker()
    do {
        try await engine.prepare()
    } catch {
        writeStderr("SmartVoices: couldn't prepare speech engine: \(error.localizedDescription)")
        return 1
    }
    let chunks = chunker.chunks(from: sanitizer.sanitize(text))
    if chunks.isEmpty {
        writeStderr("SmartVoices: Nothing to read: the text was empty after removing formatting.")
        return 1
    }
    let output: any AudioOutput = AVAudioEngineOutput()
    let start = ContinuousClock.now
    var first = true
    for (index, chunk) in chunks.enumerated() {
        let synthStart = ContinuousClock.now
        let audio: SynthesizedAudio
        do {
            audio = try await engine.synthesize(chunk.text, voiceID: nil, speed: 1.0)
        } catch {
            writeStderr("SmartVoices: Couldn't synthesize speech with \(engine.displayName): \(error.localizedDescription)")
            return 1
        }
        let synthMs = msBetween(synthStart, ContinuousClock.now)
        let audioMs = audio.sampleRate > 0 ? Int(Double(audio.samples.count) / audio.sampleRate * 1000) : 0
        print("chunk \(index): synth \(synthMs) ms, audio \(audioMs) ms")
        if first {
            first = false
            print("first audio after \(msBetween(start, ContinuousClock.now)) ms")
        }
        do {
            try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
                do {
                    try output.play(audio) {
                        continuation.resume()
                    }
                } catch {
                    continuation.resume(throwing: error)
                }
            }
        } catch {
            writeStderr("SmartVoices: Couldn't play audio: \(error.localizedDescription)")
            return 1
        }
        let pause = chunk.pauseAfter.seconds
        if pause > 0 {
            try? await Task.sleep(for: .seconds(pause))
        }
    }
    print("done")
    return 0
}

let args = CommandLine.arguments
let sayIndex = args.firstIndex(of: "--say")
let sayFileIndex = args.firstIndex(of: "--say-file")
if sayIndex != nil || sayFileIndex != nil {
    var engineID = "kokoro"
    if let engineIndex = args.firstIndex(of: "--engine") {
        if engineIndex + 1 >= args.count {
            writeStderr("SmartVoices: missing argument for --engine")
            exit(1)
        }
        engineID = args[engineIndex + 1]
    }
    if engineID != "kokoro" && engineID != "system" {
        writeStderr("SmartVoices: unknown engine '\(engineID)'. Use kokoro or system.")
        exit(1)
    }
    let text: String
    if let index = sayIndex {
        if index + 1 >= args.count {
            writeStderr("SmartVoices: missing argument for --say")
            exit(1)
        }
        text = args[index + 1]
    } else if let index = sayFileIndex {
        if index + 1 >= args.count {
            writeStderr("SmartVoices: missing argument for --say-file")
            exit(1)
        }
        let path = args[index + 1]
        do {
            text = try String(contentsOfFile: path, encoding: .utf8)
        } catch {
            writeStderr("SmartVoices: cannot read \(path): \(error.localizedDescription)")
            exit(1)
        }
    } else {
        writeStderr("SmartVoices: missing argument for --say")
        exit(1)
    }
    let engine = makeHeadlessEngine(engineID: engineID)
    Task { @MainActor in
        exit(await runHeadlessAsync(text: text, engine: engine))
    }
    RunLoop.main.run()
    exit(1)
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private var statusItem: NSStatusItem?
    private let settings = AppSettings()
    private let playback = PlaybackController(engine: SystemSpeechEngine(), output: AVAudioEngineOutput())
    private let monitor = ClipboardMonitor()
    private var engineLoading = false
    private var engineLoadFailed = false
    private var cachedVoices: [VoiceInfo] = []
    private var prepareGeneration = 0
    private var voicesGeneration = 0

    func applicationDidFinishLaunching(_ notification: Notification) {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        statusItem = item
        if let button = item.button {
            button.sendAction(on: [.leftMouseUp, .rightMouseUp])
            button.action = #selector(statusItemClicked(_:))
            button.target = self
            button.toolTip = "Smart Voices \u{2014} click to read the clipboard"
        }
        playback.engine = engineForSettings()
        playback.speed = settings.speed
        playback.voiceID = voiceForSettings()
        monitor.isEnabled = settings.watchClipboard
        startEnginePrepare()
        refreshVoices()
        updateIcon()
        observeState()
        if CommandLine.arguments.contains("--debug-click") {
            Timer.scheduledTimer(withTimeInterval: 2.0, repeats: false) { [weak self] _ in
                guard let self else { return }
                MainActor.assumeIsolated {
                    self.handleLeftClick()
                    appLogger.info("debug-click: \(String(describing: self.playback.state))")
                }
            }
        }
    }

    @objc func statusItemClicked(_ sender: Any?) {
        guard let event = NSApp.currentEvent else {
            handleLeftClick()
            return
        }
        if event.type == .rightMouseUp || event.modifierFlags.contains(.control) {
            showMenu()
        } else {
            handleLeftClick()
        }
    }

    private func handleLeftClick() {
        switch playback.state {
        case .playing, .paused, .preparing:
            playback.togglePlayPause { nil }
        case .idle:
            if let text = monitor.readText() {
                playback.play(text: text)
            } else {
                playback.lastError = "The clipboard doesn't contain any text."
            }
        }
    }

    private func showMenu() {
        guard let button = statusItem?.button else { return }
        statusItem?.menu = buildMenu()
        button.performClick(nil)
        statusItem?.menu = nil
    }

    private func buildMenu() -> NSMenu {
        let menu = NSMenu()
        let isIdle = playback.state == .idle
        let isPaused = playback.state == .paused
        let isActive = !isIdle

        let read = NSMenuItem(title: "Read Clipboard", action: #selector(readClipboard(_:)), keyEquivalent: "r")
        read.target = self
        read.isEnabled = isIdle
        menu.addItem(read)

        let pauseTitle = isPaused ? "Resume" : "Pause"
        let pauseItem = NSMenuItem(title: pauseTitle, action: #selector(togglePause(_:)), keyEquivalent: "")
        pauseItem.target = self
        pauseItem.isEnabled = isActive
        menu.addItem(pauseItem)

        let stop = NSMenuItem(title: "Stop", action: #selector(stopPlayback(_:)), keyEquivalent: "")
        stop.target = self
        stop.isEnabled = isActive
        menu.addItem(stop)

        let previous = NSMenuItem(title: "Previous Sentence", action: #selector(previousSentence(_:)), keyEquivalent: "")
        previous.target = self
        previous.isEnabled = isActive && playback.currentChunkIndex > 0
        menu.addItem(previous)

        let next = NSMenuItem(title: "Next Sentence", action: #selector(nextSentence(_:)), keyEquivalent: "")
        next.target = self
        next.isEnabled = isActive && playback.currentChunkIndex < playback.chunks.count - 1
        menu.addItem(next)

        menu.addItem(.separator())

        let speedItem = NSMenuItem(title: "Speed", action: nil, keyEquivalent: "")
        let speedMenu = NSMenu()
        for value in [0.75, 1.0, 1.25, 1.5, 1.75, 2.0] {
            let label: String
            if value == 1.0 {
                label = "1\u{00D7}"
            } else if value == 0.75 {
                label = "0.75\u{00D7}"
            } else if value == 1.25 {
                label = "1.25\u{00D7}"
            } else if value == 1.5 {
                label = "1.5\u{00D7}"
            } else if value == 1.75 {
                label = "1.75\u{00D7}"
            } else {
                label = "2\u{00D7}"
            }
            let item = NSMenuItem(title: label, action: #selector(setSpeed(_:)), keyEquivalent: "")
            item.target = self
            item.representedObject = value
            item.state = playback.speed == value ? .on : .off
            speedMenu.addItem(item)
        }
        speedItem.submenu = speedMenu
        menu.addItem(speedItem)

        let voiceItem = NSMenuItem(title: "Voice", action: nil, keyEquivalent: "")
        let voiceMenu = NSMenu()
        if cachedVoices.isEmpty {
            let loading = NSMenuItem(title: "Loading voices…", action: nil, keyEquivalent: "")
            loading.isEnabled = false
            voiceMenu.addItem(loading)
        } else {
            let currentID = voiceForSettings()
            for voice in cachedVoices {
                let item = NSMenuItem(title: voice.name, action: #selector(setVoice(_:)), keyEquivalent: "")
                item.target = self
                item.representedObject = voice.id
                item.state = voice.id == currentID ? .on : .off
                voiceMenu.addItem(item)
            }
        }
        voiceItem.submenu = voiceMenu
        menu.addItem(voiceItem)

        let engineItem = NSMenuItem(title: "Engine", action: nil, keyEquivalent: "")
        let engineMenu = NSMenu()
        for (title, id) in [("Kokoro (natural)", "kokoro"), ("macOS Voice", "system")] {
            let item = NSMenuItem(title: title, action: #selector(setEngine(_:)), keyEquivalent: "")
            item.target = self
            item.representedObject = id
            item.state = settings.engineID == id ? .on : .off
            engineMenu.addItem(item)
        }
        engineItem.submenu = engineMenu
        menu.addItem(engineItem)

        if engineLoadFailed && settings.engineID == "kokoro" {
            let fallback = NSMenuItem(title: "Use macOS Voice Instead", action: #selector(setEngine(_:)), keyEquivalent: "")
            fallback.target = self
            fallback.representedObject = "system"
            menu.addItem(fallback)
        }

        menu.addItem(.separator())

        if let error = playback.lastError {
            let full = "\u{26A0}\u{FE0E} \(error)"
            let item = NSMenuItem(title: String(full.prefix(80)), action: nil, keyEquivalent: "")
            item.isEnabled = false
            item.toolTip = full
            menu.addItem(item)
            menu.addItem(.separator())
        }

        let quit = NSMenuItem(title: "Quit Smart Voices", action: #selector(quitApp(_:)), keyEquivalent: "q")
        quit.target = self
        menu.addItem(quit)
        return menu
    }

    @objc func readClipboard(_ sender: Any?) {
        if let text = monitor.readText() {
            playback.play(text: text)
        } else {
            playback.lastError = "The clipboard doesn't contain any text."
        }
    }

    @objc func togglePause(_ sender: Any?) {
        playback.togglePlayPause { nil }
    }

    @objc func stopPlayback(_ sender: Any?) {
        playback.stop()
    }

    @objc func previousSentence(_ sender: Any?) {
        playback.skipBack()
    }

    @objc func nextSentence(_ sender: Any?) {
        playback.skipForward()
    }

    @objc func setSpeed(_ sender: NSMenuItem) {
        if let value = sender.representedObject as? Double {
            settings.speed = value
            playback.speed = settings.speed
        }
    }

    @objc func setVoice(_ sender: NSMenuItem) {
        guard let id = sender.representedObject as? String else { return }
        if playback.state != .idle {
            playback.stop()
        }
        if settings.engineID == "kokoro" {
            settings.kokoroVoiceID = id
        } else {
            settings.systemVoiceID = id
        }
        playback.voiceID = voiceForSettings()
    }

    @objc func setEngine(_ sender: NSMenuItem) {
        guard let id = sender.representedObject as? String,
            id == "kokoro" || id == "system",
            id != settings.engineID
        else {
            return
        }
        if playback.state != .idle {
            playback.stop()
        }
        settings.engineID = id
        playback.engine = engineForSettings()
        playback.voiceID = voiceForSettings()
        cachedVoices = []
        refreshVoices()
        startEnginePrepare()
    }

    private func engineForSettings() -> any SpeechEngine {
        if settings.engineID == "kokoro" {
            return KokoroSpeechEngine()
        }
        return SystemSpeechEngine()
    }

    private func voiceForSettings() -> String? {
        if settings.engineID == "kokoro" {
            return settings.kokoroVoiceID
        }
        return settings.systemVoiceID
    }

    private func startEnginePrepare() {
        prepareGeneration += 1
        let generation = prepareGeneration
        engineLoading = true
        engineLoadFailed = false
        updateIcon()
        let engine = playback.engine
        Task { @MainActor [weak self] in
            do {
                try await engine.prepare()
            } catch {
                guard let self, self.prepareGeneration == generation else { return }
                self.engineLoading = false
                self.engineLoadFailed = true
                self.playback.lastError = error.localizedDescription
                self.updateIcon()
                return
            }
            guard let self, self.prepareGeneration == generation else { return }
            self.engineLoading = false
            self.engineLoadFailed = false
            self.updateIcon()
        }
    }

    private func refreshVoices() {
        voicesGeneration += 1
        let generation = voicesGeneration
        let engine = playback.engine
        let systemOnly = settings.engineID == "system"
        Task { @MainActor [weak self] in
            var list = await engine.voices()
            if systemOnly {
                list = Array(list.filter { $0.language.lowercased().hasPrefix("en") }.prefix(30))
            }
            guard let self, self.voicesGeneration == generation else { return }
            self.cachedVoices = list
        }
    }

    @objc func quitApp(_ sender: Any?) {
        NSApp.terminate(nil)
    }

    private func observeState() {
        withObservationTracking {
            _ = playback.state
            _ = playback.lastError
            _ = monitor.hasUnreadContent
            _ = settings.watchClipboard
        } onChange: { [weak self] in
            guard let self else { return }
            MainActor.assumeIsolated {
                self.monitor.isEnabled = self.settings.watchClipboard
                if self.playback.state == .playing {
                    self.engineLoadFailed = false
                }
                self.updateIcon()
                self.observeState()
            }
        }
    }

    private func updateIcon() {
        guard let button = statusItem?.button else { return }
        let symbol: String
        let toolTip: String
        if playback.lastError != nil {
            symbol = "exclamationmark.triangle"
            toolTip = "Smart Voices \u{2014} click to read the clipboard"
        } else if engineLoading {
            symbol = "ellipsis.circle"
            toolTip = "Smart Voices \u{2014} loading the Kokoro voice model\u{2026}"
        } else {
            switch playback.state {
            case .preparing:
                symbol = "ellipsis.circle"
            case .playing:
                symbol = "pause.circle.fill"
            case .paused:
                symbol = "play.circle"
            case .idle:
                symbol = monitor.hasUnreadContent ? "play.circle.fill" : "waveform"
            }
            toolTip = "Smart Voices \u{2014} click to read the clipboard"
        }
        let image = NSImage(systemSymbolName: symbol, accessibilityDescription: "Smart Voices")
        image?.isTemplate = true
        button.image = image
        button.toolTip = toolTip
    }
}

let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let delegate = AppDelegate()
app.delegate = delegate
app.run()
