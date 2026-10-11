import AppKit
import Foundation
import SmartVoicesCore
import SwiftUI

private struct ReaderStubEngine: SpeechEngine, Sendable {
    var displayName: String { "Kokoro" }
    func prepare() async throws {}
    func voices() async -> [VoiceInfo] { [] }
    func synthesize(_ text: String, voiceID: String?, speed: Double) async throws -> SynthesizedAudio
    {
        SynthesizedAudio(samples: [], sampleRate: 24000)
    }
}

private final class ReaderStubOutput: AudioOutput {
    func play(_ audio: SynthesizedAudio, completion: @escaping @Sendable () -> Void) throws {
        completion()
    }
    func pause() {}
    func resume() {}
    func stop() {}
}

/// Render the reading panel view at 440×300 in light and dark mode.
/// `<png>` gains a `-light`/`-dark` suffix before its extension.
@MainActor
func renderReader(to png: String) -> Int32 {
    _ = NSApplication.shared
    let playback = PlaybackController(engine: ReaderStubEngine(), output: ReaderStubOutput())
    playback.chunks = [
        SpeechChunk(text: "Morning briefing for today.", pauseAfter: .medium),
        SpeechChunk(text: "First, coffee is ready in the kitchen.", pauseAfter: .short),
        SpeechChunk(text: "Second, the design review moved to noon.", pauseAfter: .short),
        SpeechChunk(text: "Third, Oslo is clear and twelve degrees.", pauseAfter: .short),
        SpeechChunk(text: "A table with 3 rows.", pauseAfter: .medium),
        SpeechChunk(text: "That's everything for now.", pauseAfter: .medium),
    ]
    playback.currentChunkIndex = 2
    playback.state = .playing
    let header = ReaderHeaderModel(name: "Heart", caption: "Kokoro · American female")
    let appearances: [(NSAppearance.Name, String)] = [(.aqua, "-light"), (.darkAqua, "-dark")]
    for (appearanceName, suffix) in appearances {
        let view = ReaderView(
            playback: playback, header: header,
            onJump: { _ in }, onTogglePlayPause: {}, onNext: {}, onPrevious: {},
            onSpeed: { _ in }, onClose: {}, onDropURLs: { _ in }, onDropText: { _ in },
            previewWordIndex: 3)
            .frame(width: 440, height: 300)
        let hosting = NSHostingView(rootView: view)
        hosting.translatesAutoresizingMaskIntoConstraints = false
        // Same composition as the real panel: SwiftUI over popover material,
        // which also gives translucent colors something to blend against.
        let effect = NSVisualEffectView()
        effect.material = .popover
        effect.state = .active
        effect.addSubview(hosting)
        NSLayoutConstraint.activate([
            hosting.leadingAnchor.constraint(equalTo: effect.leadingAnchor),
            hosting.trailingAnchor.constraint(equalTo: effect.trailingAnchor),
            hosting.topAnchor.constraint(equalTo: effect.topAnchor),
            hosting.bottomAnchor.constraint(equalTo: effect.bottomAnchor),
        ])
        effect.frame = NSRect(x: 0, y: 0, width: 440, height: 300)
        let appearance = NSAppearance(named: appearanceName)
        hosting.appearance = appearance
        effect.appearance = appearance
        let window = NSWindow(
            contentRect: NSRect(x: -10000, y: -10000, width: 440, height: 300),
            styleMask: [.titled],
            backing: .buffered,
            defer: false)
        window.appearance = appearance
        window.contentView = effect
        window.layoutIfNeeded()
        effect.layoutSubtreeIfNeeded()
        guard let rep = effect.bitmapImageRepForCachingDisplay(in: effect.bounds) else {
            writeStderr("SmartVoices: cannot render reading panel (\(suffix))")
            return 1
        }
        effect.cacheDisplay(in: effect.bounds, to: rep)
        guard let data = rep.representation(using: .png, properties: [:]) else {
            writeStderr("SmartVoices: cannot encode reading panel (\(suffix))")
            return 1
        }
        let path = suffixedPath(png, suffix: suffix)
        do {
            try data.write(to: URL(fileURLWithPath: path))
        } catch {
            writeStderr("SmartVoices: cannot write \(path): \(error.localizedDescription)")
            return 1
        }
        print(path)
    }
    return 0
}

private func suffixedPath(_ path: String, suffix: String) -> String {
    let url = URL(fileURLWithPath: path)
    let stem = url.deletingPathExtension().lastPathComponent
    let ext = url.pathExtension.isEmpty ? "png" : url.pathExtension
    return url.deletingLastPathComponent()
        .appendingPathComponent("\(stem)\(suffix).\(ext)").path
}
