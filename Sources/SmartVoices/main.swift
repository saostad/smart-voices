import AppKit
import Foundation
import Observation
import SmartVoicesCore

func writeStderr(_ message: String) {
    if let data = (message + "\n").data(using: .utf8) {
        FileHandle.standardError.write(data)
    }
}

final class ExitBox: @unchecked Sendable {
    private let lock = NSLock()
    private var code: Int32 = 0

    func set(_ value: Int32) {
        lock.lock()
        defer { lock.unlock() }
        code = value
    }

    func get() -> Int32 {
        lock.lock()
        defer { lock.unlock() }
        return code
    }
}

func runHeadless(text: String) -> Int32 {
    let semaphore = DispatchSemaphore(value: 0)
    let box = ExitBox()
    Task.detached {
        box.set(await runHeadlessAsync(text: text))
        semaphore.signal()
    }
    semaphore.wait()
    return box.get()
}

func runHeadlessAsync(text: String) async -> Int32 {
    let sanitizer = SpeechSanitizer()
    let chunker = SpeechChunker()
    let engine: any SpeechEngine = SystemSpeechEngine()
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
    for chunk in chunks {
        let audio: SynthesizedAudio
        do {
            audio = try await engine.synthesize(chunk.text, voiceID: nil, speed: 1.0)
        } catch {
            writeStderr("SmartVoices: Couldn't synthesize speech with \(engine.displayName): \(error.localizedDescription)")
            return 1
        }
        if first {
            first = false
            let latency = start.duration(to: ContinuousClock.now)
            let ms = Int(Double(latency.components.seconds) * 1000 + Double(latency.components.attoseconds) / 1_000_000_000_000_000)
            print("first audio after \(ms) ms")
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
if let index = args.firstIndex(of: "--say"), index + 1 < args.count {
    exit(runHeadless(text: args[index + 1]))
} else if let index = args.firstIndex(of: "--say-file"), index + 1 < args.count {
    let path = args[index + 1]
    do {
        let text = try String(contentsOfFile: path, encoding: .utf8)
        exit(runHeadless(text: text))
    } catch {
        writeStderr("SmartVoices: cannot read \(path): \(error.localizedDescription)")
        exit(1)
    }
} else if args.contains("--say") || args.contains("--say-file") {
    writeStderr("SmartVoices: missing argument for \(args.contains("--say") ? "--say" : "--say-file")")
    exit(1)
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private var statusItem: NSStatusItem?
    private let playback = PlaybackController(engine: SystemSpeechEngine(), output: AVAudioEngineOutput())
    private let monitor = ClipboardMonitor()

    func applicationDidFinishLaunching(_ notification: Notification) {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        statusItem = item
        if let button = item.button {
            button.sendAction(on: [.leftMouseUp, .rightMouseUp])
            button.action = #selector(statusItemClicked(_:))
            button.target = self
            button.toolTip = "Smart Voices \u{2014} click to read the clipboard"
        }
        updateIcon()
        observeState()
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
            playback.speed = value
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
        } onChange: { [weak self] in
            guard let self else { return }
            MainActor.assumeIsolated {
                self.updateIcon()
                self.observeState()
            }
        }
    }

    private func updateIcon() {
        guard let button = statusItem?.button else { return }
        let symbol: String
        if playback.lastError != nil {
            symbol = "exclamationmark.triangle"
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
        }
        let image = NSImage(systemSymbolName: symbol, accessibilityDescription: "Smart Voices")
        image?.isTemplate = true
        button.image = image
        button.toolTip = "Smart Voices \u{2014} click to read the clipboard"
    }
}

let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let delegate = AppDelegate()
app.delegate = delegate
app.run()
