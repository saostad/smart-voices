import AVFoundation
import Foundation

/// Plays synthesized audio.
public protocol AudioOutput: AnyObject {
    func play(_ audio: SynthesizedAudio, completion: @escaping @Sendable () -> Void) throws
    func pause()
    func resume()
    func stop()
}

/// Errors from audio playback.
public enum AudioOutputError: Error, Sendable, Equatable {
    case engineStartFailed(String)
    case invalidAudio(String)
}

extension AudioOutputError: LocalizedError {
    public var errorDescription: String? {
        switch self {
        case .engineStartFailed(let message), .invalidAudio(let message):
            return message
        }
    }
}

/// Plays audio through AVAudioEngine.
public final class AVAudioEngineOutput: AudioOutput {
    private let engine = AVAudioEngine()
    private let player = AVAudioPlayerNode()
    private var currentSampleRate: Double?

    public init() {
        engine.attach(player)
    }

    public func play(_ audio: SynthesizedAudio, completion: @escaping @Sendable () -> Void) throws {
        if audio.samples.isEmpty {
            DispatchQueue.global().async(execute: completion)
            return
        }
        if currentSampleRate != audio.sampleRate {
            engine.stop()
            engine.disconnectNodeOutput(player)
            guard let format = AVAudioFormat(standardFormatWithSampleRate: audio.sampleRate, channels: 1) else {
                throw AudioOutputError.invalidAudio("Couldn't play audio: unsupported sample rate \(audio.sampleRate).")
            }
            engine.connect(player, to: engine.mainMixerNode, format: format)
            currentSampleRate = audio.sampleRate
        }
        if !engine.isRunning {
            do {
                try engine.start()
            } catch {
                throw AudioOutputError.engineStartFailed("Couldn't start audio engine: \(error.localizedDescription)")
            }
        }
        guard let format = AVAudioFormat(standardFormatWithSampleRate: audio.sampleRate, channels: 1) else {
            throw AudioOutputError.invalidAudio("Couldn't play audio: unsupported sample rate \(audio.sampleRate).")
        }
        guard let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(audio.samples.count)) else {
            throw AudioOutputError.invalidAudio("Couldn't play audio: couldn't allocate playback buffer.")
        }
        buffer.frameLength = AVAudioFrameCount(audio.samples.count)
        if let channelData = buffer.floatChannelData {
            audio.samples.withUnsafeBufferPointer { source in
                if let base = source.baseAddress, !audio.samples.isEmpty {
                    channelData[0].update(from: base, count: audio.samples.count)
                }
            }
        }
        player.scheduleBuffer(buffer, at: nil, options: []) {
            completion()
        }
        player.play()
    }

    public func pause() {
        player.pause()
    }

    public func resume() {
        player.play()
    }

    public func stop() {
        player.stop()
    }
}
