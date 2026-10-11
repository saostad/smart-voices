import AVFoundation
import Foundation

/// macOS system voice engine.
public struct SystemSpeechEngine: SpeechEngine, Sendable {
    public var displayName: String { "macOS speech" }

    public init() {}

    public func prepare() async throws {}

    public func voices() async -> [VoiceInfo] {
        AVSpeechSynthesisVoice.speechVoices()
            .map { VoiceInfo(id: $0.identifier, name: $0.name, language: $0.language) }
            .sorted {
                let lhsEN = $0.language.lowercased().hasPrefix("en")
                let rhsEN = $1.language.lowercased().hasPrefix("en")
                if lhsEN != rhsEN {
                    return lhsEN
                }
                return $0.name.localizedCompare($1.name) == .orderedAscending
            }
    }

    public func synthesize(_ text: String, voiceID: String?, speed: Double) async throws -> SynthesizedAudio {
        let utterance = AVSpeechUtterance(string: text)
        if let voiceID, let voice = AVSpeechSynthesisVoice(identifier: voiceID) {
            utterance.voice = voice
        }
        let scaled = AVSpeechUtteranceDefaultSpeechRate * Float(speed)
        utterance.rate = min(AVSpeechUtteranceMaximumSpeechRate, max(AVSpeechUtteranceMinimumSpeechRate, scaled))
        let holder = WriteHolder(utterance: utterance)
        let buffers = await holder.run()
        var samples: [Float] = []
        var sampleRate = 0.0
        for buffer in buffers {
            if sampleRate == 0 {
                sampleRate = buffer.format.sampleRate
            }
            samples.append(contentsOf: try monoSamples(from: buffer))
        }
        if samples.isEmpty {
            throw SpeechEngineError.noAudio("macOS speech produced no audio for: \(String(text.prefix(40)))")
        }
        return SynthesizedAudio(samples: samples, sampleRate: sampleRate)
    }

    private func monoSamples(from buffer: AVAudioPCMBuffer) throws -> [Float] {
        if buffer.format.commonFormat == .pcmFormatFloat32,
            let channelData = buffer.floatChannelData
        {
            let channels = Int(buffer.format.channelCount)
            let frames = Int(buffer.frameLength)
            if channels == 1 {
                return Array(UnsafeBufferPointer(start: channelData[0], count: frames))
            }
            var mono: [Float] = []
            mono.reserveCapacity(frames)
            for frame in 0..<frames {
                var sum: Float = 0
                for channel in 0..<channels {
                    sum += channelData[channel][frame]
                }
                mono.append(sum / Float(channels))
            }
            return mono
        }
        guard let outputFormat = AVAudioFormat(
            commonFormat: .pcmFormatFloat32,
            sampleRate: buffer.format.sampleRate,
            channels: 1,
            interleaved: false
        ) else {
            throw SpeechEngineError.conversionFailed("macOS speech produced audio in an unsupported format.")
        }
        guard let converter = AVAudioConverter(from: buffer.format, to: outputFormat) else {
            throw SpeechEngineError.conversionFailed("macOS speech produced audio in an unsupported format.")
        }
        guard let outBuffer = AVAudioPCMBuffer(pcmFormat: outputFormat, frameCapacity: buffer.frameLength) else {
            throw SpeechEngineError.conversionFailed("macOS speech audio conversion failed.")
        }
        do {
            try converter.convert(to: outBuffer, from: buffer)
        } catch {
            throw SpeechEngineError.conversionFailed("macOS speech audio conversion failed: \(error.localizedDescription)")
        }
        guard let channelData = outBuffer.floatChannelData else {
            return []
        }
        return Array(UnsafeBufferPointer(start: channelData[0], count: Int(outBuffer.frameLength)))
    }
}

private final class WriteHolder: @unchecked Sendable {
    private let utterance: AVSpeechUtterance
    private let synthesizer = AVSpeechSynthesizer()
    private let lock = NSLock()
    private var buffers: [AVAudioPCMBuffer] = []
    private var resumed = false

    init(utterance: AVSpeechUtterance) {
        self.utterance = utterance
    }

    func run() async -> [AVAudioPCMBuffer] {
        await withCheckedContinuation { continuation in
            synthesizer.write(utterance) { [weak self] buffer in
                guard let self else { return }
                guard let pcm = buffer as? AVAudioPCMBuffer else { return }
                self.lock.lock()
                if pcm.frameLength == 0 {
                    let shouldResume = !self.resumed
                    self.resumed = true
                    let collected = self.buffers
                    self.lock.unlock()
                    if shouldResume {
                        continuation.resume(returning: collected)
                    }
                } else {
                    self.buffers.append(pcm)
                    self.lock.unlock()
                }
            }
        }
    }
}
