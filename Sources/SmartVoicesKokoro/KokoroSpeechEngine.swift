import FluidAudio
import Foundation
import SmartVoicesCore

/// Kokoro neural voice engine, running on the Apple Neural Engine.
public actor KokoroSpeechEngine: SpeechEngine {
    public static let defaultVoiceID = "af_heart"

    public nonisolated var displayName: String { "Kokoro" }

    private let manager: KokoroAneManager
    private var prepareTask: Task<Void, Error>?

    public init() {
        self.manager = KokoroAneManager()
    }

    /// Load the model (downloading on first run) and warm up. Idempotent and
    /// safe to call concurrently: initialize runs once. Failures are not cached.
    public func prepare() async throws {
        if let existing = prepareTask {
            try await existing.value
            return
        }
        let task = Task<Void, Error> {
            try await self.runPrepare()
        }
        prepareTask = task
        do {
            try await task.value
        } catch {
            prepareTask = nil
            throw error
        }
    }

    public func voices() async -> [VoiceInfo] {
        let infos = KokoroAneConstants.englishVoices.compactMap { id -> VoiceInfo? in
            guard id.hasPrefix("af_") || id.hasPrefix("am_") || id.hasPrefix("bf_") || id.hasPrefix("bm_"),
                let name = KokoroVoiceCatalog.displayName(for: id)
            else {
                return nil
            }
            return VoiceInfo(id: id, name: name, language: id.hasPrefix("a") ? "en-US" : "en-GB")
        }
        return infos.sorted {
            if $0.id == Self.defaultVoiceID { return true }
            if $1.id == Self.defaultVoiceID { return false }
            return $0.name.localizedCompare($1.name) == .orderedAscending
        }
    }

    public func synthesize(_ text: String, voiceID: String?, speed: Double) async throws -> SynthesizedAudio {
        try await prepare()
        let voice = voiceID ?? Self.defaultVoiceID
        guard KokoroAneConstants.englishVoices.contains(voice) else {
            throw SpeechEngineError.unknownVoice("Kokoro has no voice named '\(voice)'.")
        }
        let result = try await manager.synthesizeDetailed(text: text, voice: voice, speed: Float(speed))
        return SynthesizedAudio(samples: result.samples, sampleRate: Double(result.sampleRate))
    }

    private func runPrepare() async throws {
        do {
            try await manager.initialize()
            _ = try await manager.synthesizeDetailed(text: "Ready.", voice: Self.defaultVoiceID, speed: 1.0)
        } catch {
            throw SpeechEngineError.engineUnavailable(
                "Couldn't load the Kokoro voice model: \(error.localizedDescription). The first run downloads about 120 MB, so check your internet connection."
            )
        }
    }
}
