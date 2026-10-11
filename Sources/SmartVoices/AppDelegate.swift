import AppKit
import Foundation
import Observation
import os
import SmartVoicesCore
import SmartVoicesKokoro
import UniformTypeIdentifiers

let appLogger = Logger(subsystem: "dev.smartvoices", category: "app")

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private var statusItem: NSStatusItem?
    let settings = AppSettings()
    let playback = PlaybackController(engine: SystemSpeechEngine(), output: AVAudioEngineOutput())
    let history = ReadingHistory()
    let exporter = AudioExporter()
    private let nowPlaying = NowPlayingController()
    private let monitor = ClipboardMonitor()
    let hotKeyCenter = HotKeyCenter()
    let voiceModel = VoiceModel()
    let loginItem: any LoginItemControlling = SMAppServiceLoginItem()
    private let settingsWindowController = SettingsWindowController()
    private let servicesProvider = ServicesProvider()
    private let reader = ReaderPanelController()
    private var readerHideTimer: Timer?
    private var engineLoading = false
    var engineLoadFailed = false
    var cachedVoices: [VoiceInfo] = []
    private var routedVoiceName: String?
    private var routedReaderCaption: String?
    let sleepTimer = SleepTimer()
    var sleepMinutes: Int?
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
        playback.sanitizerOptions = settings.sanitizerOptions
        playback.pronunciationDictionary = PronunciationDictionary(entries: settings.pronunciations)
        monitor.isEnabled = settings.watchClipboard
        monitor.ignoredAppBundleIDs = settings.ignoredAppBundleIDs
        monitor.minimumReadyLength = settings.minimumReadyLength
        hotKeyCenter.onAction = { [weak self] action in
            self?.handleHotKey(action)
        }
        hotKeyCenter.updateBindings(settings.hotKeys)
        nowPlaying.onPlay = { [weak self] in
            guard let self else { return }
            if self.playback.state == .paused {
                self.playback.resume()
            } else if self.playback.state == .idle {
                self.readClipboardAndPlay()
            }
        }
        nowPlaying.onPause = { [weak self] in self?.playback.pause() }
        nowPlaying.onToggle = { [weak self] in self?.handleLeftClick() }
        nowPlaying.onNext = { [weak self] in self?.playback.skipForward() }
        nowPlaying.onPrevious = { [weak self] in self?.playback.skipBack() }
        nowPlaying.onStop = { [weak self] in self?.playback.stop() }
        servicesProvider.onReadSelection = { [weak self] text in self?.playAndRecord(text) }
        servicesProvider.onServiceError = { [weak self] message in
            self?.playback.lastError = message
        }
        NSApp.servicesProvider = servicesProvider
        NSUpdateDynamicServices()
        reader.onDropURLs = { [weak self] urls in self?.readDroppedURLs(urls) }
        reader.onDropText = { [weak self] text in self?.playAndRecord(text) }
        startEnginePrepare()
        refreshVoices()
        let header = readerHeader()
        reader.setHeader(name: header.name, caption: header.caption)
        if let loadError = history.loadError {
            playback.lastError =
                "Couldn't save reading history: \(loadError.localizedDescription)"
        }
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
            readClipboardAndPlay()
        }
    }

    private func handleHotKey(_ action: HotKeyAction) {
        switch action {
        case .playPause:
            handleLeftClick()
        case .stop:
            playback.stop()
        case .nextSentence:
            playback.skipForward()
        case .previousSentence:
            playback.skipBack()
        }
    }

    private func readClipboardAndPlay() {
        switch monitor.readText() {
        case .text(let text):
            playAndRecord(text)
        case .empty:
            playback.lastError = "The clipboard doesn't contain any text."
        case .tooShort(let count):
            playback.lastError =
                "The clipboard text is only \(count) characters. Copy something longer, or lower the minimum in Settings → General."
        }
    }

    /// Play text from any source, recording it in history when playback starts.
    private func playAndRecord(_ text: String) {
        playRouted(text)
        if playback.state != .idle {
            recordHistory(text)
        }
    }

    /// Play text, switching to a matching macOS voice for non-English text
    /// when the Kokoro engine is selected and auto-switching is on.
    /// Voice/pronunciation previews pass rewrite: false so the exact sample is heard.
    private func playRouted(_ text: String, rewrite: Bool = true) {
        routedVoiceName = nil
        routedReaderCaption = nil
        playback.rewriter =
            (rewrite && settings.conversationalMode)
            ? ConversationRewriteSupport.makeAttemptedRewriter() : nil
        var engineOverride: (any SpeechEngine)?
        var voiceOverride: String?
        if settings.engineID == "kokoro", settings.autoSwitchLanguage,
            let detected = LanguageRouter.dominantLanguage(of: text),
            detected.code != "en"
        {
            guard let voice = SystemVoicePicker.bestVoice(forLanguage: detected.code) else {
                playback.lastError = LanguageRouting.missingVoiceMessage(for: detected.code)
                return
            }
            let languageName =
                Locale.current.localizedString(forLanguageCode: detected.code) ?? detected.code
            engineOverride = SystemSpeechEngine()
            voiceOverride = voice.id
            routedVoiceName = voice.name
            routedReaderCaption = "\(languageName) · macOS Voice (Kokoro speaks English)"
            appLogger.notice(
                "switched to macOS voice \(voice.name, privacy: .public) for \(languageName, privacy: .public) text (Kokoro speaks English)"
            )
        }
        playback.play(text: text, engineOverride: engineOverride, voiceOverride: voiceOverride)
    }

    private func recordHistory(_ text: String) {
        guard settings.keepHistory else { return }
        do {
            try history.record(text)
        } catch {
            playback.lastError = "Couldn't save reading history: \(error.localizedDescription)"
        }
    }

    func clearHistory() {
        do {
            try history.clear()
        } catch {
            playback.lastError = "Couldn't save reading history: \(error.localizedDescription)"
        }
    }

    private func showMenu() {
        guard let button = statusItem?.button else { return }
        statusItem?.menu = buildMenu()
        button.performClick(nil)
        statusItem?.menu = nil
    }

    @objc func readClipboard(_ sender: Any?) {
        readClipboardAndPlay()
    }

    @objc func replayHistory(_ sender: NSMenuItem) {
        guard let id = sender.representedObject as? String,
            let item = history.items.first(where: { $0.id.uuidString == id })
        else {
            return
        }
        playAndRecord(item.fullText)
    }

    @objc func readFile(_ sender: Any?) {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = DocumentTextLoader.supportedExtensions.compactMap {
            UTType(filenameExtension: $0)
        }
        panel.allowsMultipleSelection = false
        guard panel.runModal() == .OK, let url = panel.url else { return }
        readFileAndPlay(url)
    }

    private func readFileAndPlay(_ url: URL) {
        let name = url.lastPathComponent
        let text: String
        do {
            text = try DocumentTextLoader.load(url: url)
        } catch {
            playback.lastError = "Couldn't open \(name): \(error.localizedDescription)"
            return
        }
        if text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            playback.lastError = "\(name) has no readable text."
            return
        }
        playAndRecord(text)
    }

    private func readDroppedURLs(_ urls: [URL]) {
        var texts: [String] = []
        for url in urls {
            do {
                texts.append(try DocumentTextLoader.load(url: url))
            } catch {
                playback.lastError =
                    "Couldn't open \(url.lastPathComponent): \(error.localizedDescription)"
                return
            }
        }
        let combined = texts.joined(separator: "\n\n").trimmingCharacters(
            in: .whitespacesAndNewlines)
        if combined.isEmpty {
            playback.lastError = "\(urls.first?.lastPathComponent ?? "file") has no readable text."
            return
        }
        playAndRecord(combined)
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

    @objc func exportClipboardAsAudio(_ sender: Any?) {
        let text: String
        switch monitor.readText() {
        case .text(let value):
            text = value
        case .empty:
            playback.lastError = "The clipboard doesn't contain any text."
            return
        case .tooShort(let count):
            playback.lastError =
                "The clipboard text is only \(count) characters. Copy something longer, or lower the minimum in Settings → General."
            return
        }
        let panel = NSSavePanel()
        if let m4a = UTType(filenameExtension: "m4a") {
            panel.allowedContentTypes = [m4a]
        }
        panel.nameFieldStringValue = defaultExportName()
        panel.canCreateDirectories = true
        guard panel.runModal() == .OK, let url = panel.url else { return }
        let engine = playback.engine
        let voiceID = voiceForSettings()
        let speed = settings.speed
        let options = settings.sanitizerOptions
        let pronunciations = settings.pronunciations
        let rewriter: (any ConversationRewriter)? =
            settings.conversationalMode
            ? ConversationRewriteSupport.makeAttemptedRewriter() : nil
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                _ = try await self.exporter.export(
                    text: text, engine: engine, voiceID: voiceID, speed: speed,
                    sanitizerOptions: options, pronunciations: pronunciations,
                    rewriter: rewriter, to: url)
                if let notice = self.exporter.lastNotice {
                    self.playback.lastNotice = notice
                }
                NSWorkspace.shared.activateFileViewerSelecting([url])
            } catch {
                self.playback.lastError = "Couldn't export audio: \(error.localizedDescription)"
            }
        }
    }

    private func defaultExportName() -> String {
        let formatter = DateFormatter()
        formatter.dateFormat = "yyyy-MM-dd HHmm"
        return "Smart Voices \(formatter.string(from: Date())).m4a"
    }

    @objc func setSpeed(_ sender: NSMenuItem) {
        if let value = sender.representedObject as? Double {
            settings.speed = value
            playback.speed = settings.speed
        }
    }

    @objc func setSleepTimer(_ sender: NSMenuItem) {
        guard let minutes = sender.representedObject as? Int else { return }
        if minutes <= 0 {
            sleepTimer.cancel()
            sleepMinutes = nil
            return
        }
        sleepMinutes = minutes
        sleepTimer.start(seconds: TimeInterval(minutes * 60)) { [weak self] in
            guard let self else { return }
            MainActor.assumeIsolated {
                self.sleepMinutes = nil
                self.playback.stop()
                appLogger.notice("sleep timer fired; playback stopped")
            }
        }
    }

    @objc func setVoice(_ sender: NSMenuItem) {
        guard let id = sender.representedObject as? String else { return }
        setVoice(id: id)
    }

    func setVoice(id: String?) {
        if playback.state != .idle {
            playback.stop()
        }
        if settings.engineID == "kokoro" {
            if let id {
                settings.kokoroVoiceID = id
            }
        } else {
            settings.systemVoiceID = id
        }
        playback.voiceID = voiceForSettings()
    }

    @objc func setEngine(_ sender: NSMenuItem) {
        guard let id = sender.representedObject as? String else { return }
        setEngine(id: id)
    }

    func setEngine(id: String) {
        guard id == "kokoro" || id == "system", id != settings.engineID else {
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

    func previewVoice() {
        let currentID = voiceForSettings()
        let name =
            cachedVoices.first(where: { $0.id == currentID })?.name ?? currentID ?? "Default"
        playRouted("Hi, I'm \(name). This is how I sound.", rewrite: false)
    }

    func previewPronunciation(_ spoken: String) {
        playRouted(spoken, rewrite: false)
    }

    @objc func showReaderPanel(_ sender: Any?) {
        readerHideTimer?.invalidate()
        readerHideTimer = nil
        presentReader()
    }

    private func presentReader() {
        reader.show(
            playback: playback, header: readerHeader(),
            onJump: { [weak self] index in self?.playback.jump(to: index) },
            onTogglePlayPause: { [weak self] in self?.handleLeftClick() },
            onNext: { [weak self] in self?.playback.skipForward() },
            onPrevious: { [weak self] in self?.playback.skipBack() },
            onSpeed: { [weak self] value in
                self?.settings.speed = value
                if let self {
                    self.playback.speed = self.settings.speed
                }
            },
            onClose: { [weak self] in
                self?.readerHideTimer?.invalidate()
                self?.readerHideTimer = nil
                self?.reader.hide()
            })
    }

    private func readerHeader() -> (name: String, caption: String) {
        if let caption = routedReaderCaption {
            return (routedVoiceName ?? "macOS Voice", caption)
        }
        let currentID = voiceForSettings()
        let voiceName =
            cachedVoices.first(where: { $0.id == currentID })?.name ?? currentID ?? "Default"
        let engineLabel = settings.engineID == "kokoro" ? "Kokoro" : "macOS Voice"
        let parts = voiceName.split(separator: "—", maxSplits: 1).map {
            $0.trimmingCharacters(in: .whitespaces)
        }
        if parts.count == 2 {
            return (parts[0], "\(engineLabel) · \(parts[1])")
        }
        return (voiceName, engineLabel)
    }

    private func updateReaderPanel() {
        let active =
            playback.state == .playing || playback.state == .preparing
            || playback.state == .paused
        if active {
            readerHideTimer?.invalidate()
            readerHideTimer = nil
            if settings.showReaderPanel {
                presentReader()
            }
        } else if reader.isVisible {
            scheduleReaderHide()
        }
    }

    private func scheduleReaderHide() {
        readerHideTimer?.invalidate()
        readerHideTimer = Timer.scheduledTimer(withTimeInterval: 3.0, repeats: false) {
            [weak self] _ in
            guard let self else { return }
            MainActor.assumeIsolated {
                if self.reader.isMouseOver {
                    self.scheduleReaderHide()
                } else {
                    self.reader.hide()
                }
            }
        }
    }

    @objc func openSettings(_ sender: Any?) {
        showSettings()
    }

    func showSettings() {
        settingsWindowController.show(
            settings: settings,
            voiceModel: voiceModel,
            hotKeyCenter: hotKeyCenter,
            loginItem: loginItem,
            onSelectEngine: { [weak self] id in Task { @MainActor in self?.setEngine(id: id) } },
            onSelectVoice: { [weak self] id in Task { @MainActor in self?.setVoice(id: id) } },
            onPreviewVoice: { [weak self] in Task { @MainActor in self?.previewVoice() } },
            onPreviewPronunciation: { [weak self] text in
                Task { @MainActor in self?.previewPronunciation(text) }
            },
            onClearHistory: { [weak self] in Task { @MainActor in self?.clearHistory() } })
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
        voiceModel.status = .loading
        let start = ContinuousClock.now
        Task { @MainActor [weak self] in
            do {
                try await engine.prepare()
            } catch {
                guard let self, self.prepareGeneration == generation else { return }
                self.engineLoading = false
                self.engineLoadFailed = true
                self.playback.lastError = error.localizedDescription
                self.voiceModel.status = .failed(error.localizedDescription)
                appLogger.notice("engine prepare failed: \(error.localizedDescription, privacy: .public)")
                self.updateIcon()
                return
            }
            guard let self, self.prepareGeneration == generation else { return }
            self.engineLoading = false
            self.engineLoadFailed = false
            self.voiceModel.status = .ready
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
            self.voiceModel.voices = list
            let header = self.readerHeader()
            self.reader.setHeader(name: header.name, caption: header.caption)
        }
    }

    @objc func quitApp(_ sender: Any?) {
        NSApp.terminate(nil)
    }

    private func observeState() {
        withObservationTracking {
            _ = playback.state
            _ = playback.lastError
            _ = playback.currentChunkIndex
            _ = playback.chunks
            _ = exporter.isExporting
            _ = exporter.completedChunks
            _ = monitor.hasUnreadContent
            _ = settings.showReaderPanel
            _ = settings.watchClipboard
            _ = settings.autoReadClipboard
            _ = settings.ignoredAppBundleIDs
            _ = settings.minimumReadyLength
            _ = settings.codeBlocks
            _ = settings.tables
            _ = settings.bareURLs
            _ = settings.maxTableRowsToRead
            _ = settings.pronunciations
            _ = settings.hotKeys
            _ = settings.speed
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
                self.monitor.ignoredAppBundleIDs = self.settings.ignoredAppBundleIDs
                self.monitor.minimumReadyLength = self.settings.minimumReadyLength
                self.playback.sanitizerOptions = self.settings.sanitizerOptions
                self.playback.pronunciationDictionary = PronunciationDictionary(
                    entries: self.settings.pronunciations)
                self.playback.speed = self.settings.speed
                self.hotKeyCenter.updateBindings(self.settings.hotKeys)
                if self.playback.state == .playing {
                    self.engineLoadFailed = false
                }
                if self.playback.state == .idle {
                    self.routedVoiceName = nil
                    self.routedReaderCaption = nil
                }
                if self.settings.autoReadClipboard, self.monitor.hasUnreadContent,
                    self.playback.state == .idle, !self.exporter.isExporting
                {
                    self.readClipboardAndPlay()
                }
                self.updateReaderPanel()
                self.updateNowPlaying()
                self.updateIcon()
            }
        }
    }

    private func updateNowPlaying() {
        nowPlaying.update(
            state: playback.state, chunks: playback.chunks,
            index: playback.currentChunkIndex, speed: playback.speed)
    }

    private func updateIcon() {
        guard let button = statusItem?.button else { return }
        let symbol: String
        let toolTip: String
        if playback.lastError != nil {
            symbol = "exclamationmark.triangle"
            toolTip = "Smart Voices \u{2014} click to read the clipboard"
        } else if exporter.isExporting {
            symbol = "ellipsis.circle"
            toolTip =
                "Smart Voices \u{2014} exporting audio (\(exporter.completedChunks) of \(exporter.totalChunks))\u{2026}"
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
