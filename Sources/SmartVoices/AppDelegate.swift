import AppKit
import Foundation
import Observation
import os
import SmartVoicesCore
import SmartVoicesKokoro

let appLogger = Logger(subsystem: "dev.smartvoices", category: "app")

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private var statusItem: NSStatusItem?
    let settings = AppSettings()
    let playback = PlaybackController(engine: SystemSpeechEngine(), output: AVAudioEngineOutput())
    private let monitor = ClipboardMonitor()
    private var engineLoading = false
    var engineLoadFailed = false
    var cachedVoices: [VoiceInfo] = []
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
                    appLogger.notice("debug-click: \(String(describing: self.playback.state), privacy: .public)")
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

    func voiceForSettings() -> String? {
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
        appLogger.notice("engine prepare started (\(engine.displayName, privacy: .public))")
        let start = ContinuousClock.now
        Task { @MainActor [weak self] in
            do {
                try await engine.prepare()
            } catch {
                guard let self, self.prepareGeneration == generation else { return }
                self.engineLoading = false
                self.engineLoadFailed = true
                self.playback.lastError = error.localizedDescription
                appLogger.notice("engine prepare failed: \(error.localizedDescription, privacy: .public)")
                self.updateIcon()
                return
            }
            guard let self, self.prepareGeneration == generation else { return }
            self.engineLoading = false
            self.engineLoadFailed = false
            appLogger.notice("engine prepare finished in \(msBetween(start, ContinuousClock.now), privacy: .public) ms")
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
            // onChange fires before the new value is stored: re-register now, but
            // read state asynchronously so the new values are visible.
            MainActor.assumeIsolated {
                self.observeState()
            }
            Task { @MainActor [weak self] in
                guard let self else { return }
                self.monitor.isEnabled = self.settings.watchClipboard
                if self.playback.state == .playing {
                    self.engineLoadFailed = false
                }
                self.updateIcon()
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
