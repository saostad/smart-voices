import Foundation

/// Mono PCM audio produced by a speech engine.
public struct SynthesizedAudio: Sendable {
    public var samples: [Float]
    public var sampleRate: Double

    public init(samples: [Float], sampleRate: Double) {
        self.samples = samples
        self.sampleRate = sampleRate
    }
}

/// A voice offered by a speech engine.
public struct VoiceInfo: Sendable, Hashable, Identifiable {
    public var id: String
    public var name: String
    public var language: String

    public init(id: String, name: String, language: String) {
        self.id = id
        self.name = name
        self.language = language
    }
}

/// Errors from speech synthesis.
public enum SpeechEngineError: Error, Sendable, Equatable {
    case noAudio(String)
    case conversionFailed(String)
    case engineUnavailable(String)
    case unknownVoice(String)
}

extension SpeechEngineError: LocalizedError {
    public var errorDescription: String? {
        switch self {
        case .noAudio(let message),
            .conversionFailed(let message),
            .engineUnavailable(let message),
            .unknownVoice(let message):
            return message
        }
    }
}

/// Converts text to audio.
public protocol SpeechEngine: Sendable {
    var displayName: String { get }
    func prepare() async throws
    func voices() async -> [VoiceInfo]
    func synthesize(_ text: String, voiceID: String?, speed: Double) async throws -> SynthesizedAudio
}
