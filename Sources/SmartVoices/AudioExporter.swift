import AVFoundation
import Foundation
import SmartVoicesCore

/// Errors from audio export.
enum AudioExportError: Error, Sendable, Equatable {
    case nothingToRead
    case engineFailed(String)
    case conversionFailed(String)
    case writeFailed(String)
}

extension AudioExportError: LocalizedError {
    var errorDescription: String? {
        switch self {
        case .nothingToRead:
            return "the text was empty after removing formatting."
        case .engineFailed(let message),
            .conversionFailed(let message),
            .writeFailed(let message):
            return message
        }
    }
}

/// Synthesizes text and writes it as an AAC .m4a file.
@MainActor @Observable
final class AudioExporter {
    private(set) var isExporting = false
    private(set) var completedChunks = 0
    private(set) var totalChunks = 0

    /// Synthesize every chunk and write AAC 24 kHz mono to `url`.
    /// Returns the audio duration in seconds.
    func export(
        text: String, engine: any SpeechEngine, voiceID: String?, speed: Double,
        sanitizerOptions: SanitizerOptions, to url: URL
    ) async throws -> Double {
        do {
            try await engine.prepare()
        } catch {
            throw AudioExportError.engineFailed(
                "\(engine.displayName): \(error.localizedDescription)")
        }
        let chunks = SpeechChunker().chunks(
            from: SpeechSanitizer(options: sanitizerOptions).sanitize(text))
        if chunks.isEmpty {
            throw AudioExportError.nothingToRead
        }
        isExporting = true
        totalChunks = chunks.count
        completedChunks = 0
        defer {
            isExporting = false
        }
        var audios: [SynthesizedAudio] = []
        var pauses: [Double] = []
        for chunk in chunks {
            do {
                audios.append(
                    try await engine.synthesize(chunk.text, voiceID: voiceID, speed: speed))
            } catch {
                throw AudioExportError.engineFailed(
                    "\(engine.displayName): \(error.localizedDescription)")
            }
            pauses.append(chunk.pauseAfter.seconds)
            completedChunks += 1
        }
        let stitched: SynthesizedAudio
        do {
            stitched = try AudioStitcher.stitch(audios, pauses: pauses)
        } catch {
            throw AudioExportError.engineFailed(error.localizedDescription)
        }
        let mono24k = try resampleTo24kMono(stitched)
        try writeAAC(mono24k, to: url)
        return Double(mono24k.samples.count) / 24000
    }

    private func resampleTo24kMono(_ audio: SynthesizedAudio) throws -> SynthesizedAudio {
        if audio.sampleRate == 24000 {
            return audio
        }
        guard audio.sampleRate > 0, !audio.samples.isEmpty else {
            throw AudioExportError.conversionFailed("unsupported audio format.")
        }
        guard let inFormat = AVAudioFormat(
            standardFormatWithSampleRate: audio.sampleRate, channels: 1),
            let outFormat = AVAudioFormat(standardFormatWithSampleRate: 24000, channels: 1),
            let converter = AVAudioConverter(from: inFormat, to: outFormat),
            let inBuffer = AVAudioPCMBuffer(
                pcmFormat: inFormat, frameCapacity: AVAudioFrameCount(audio.samples.count))
        else {
            throw AudioExportError.conversionFailed("couldn't set up sample-rate conversion.")
        }
        inBuffer.frameLength = AVAudioFrameCount(audio.samples.count)
        copySamples(audio.samples, into: inBuffer)
        let outCapacity = AVAudioFrameCount(
            (Double(audio.samples.count) * 24000 / audio.sampleRate).rounded(.up) + 1)
        guard let outBuffer = AVAudioPCMBuffer(pcmFormat: outFormat, frameCapacity: outCapacity)
        else {
            throw AudioExportError.conversionFailed("couldn't allocate the conversion buffer.")
        }
        var consumed = false
        var convertError: NSError?
        let status = converter.convert(to: outBuffer, error: &convertError) {
            _, outStatus in
            if consumed {
                outStatus.pointee = .noDataNow
                return nil
            }
            consumed = true
            outStatus.pointee = .haveData
            return inBuffer
        }
        if status == .error {
            throw AudioExportError.conversionFailed(
                convertError?.localizedDescription ?? "sample-rate conversion failed.")
        }
        return SynthesizedAudio(samples: samples(from: outBuffer), sampleRate: 24000)
    }

    private func writeAAC(_ audio: SynthesizedAudio, to url: URL) throws {
        let settings: [String: Any] = [
            AVFormatIDKey: kAudioFormatMPEG4AAC,
            AVSampleRateKey: 24000,
            AVNumberOfChannelsKey: 1,
            AVEncoderAudioQualityKey: AVAudioQuality.high.rawValue,
        ]
        do {
            let file = try AVAudioFile(forWriting: url, settings: settings)
            guard let format = AVAudioFormat(standardFormatWithSampleRate: 24000, channels: 1),
                let buffer = AVAudioPCMBuffer(
                    pcmFormat: format,
                    frameCapacity: AVAudioFrameCount(max(audio.samples.count, 1)))
            else {
                throw AudioExportError.writeFailed("couldn't allocate the export buffer.")
            }
            buffer.frameLength = AVAudioFrameCount(audio.samples.count)
            copySamples(audio.samples, into: buffer)
            try file.write(from: buffer)
        } catch let error as AudioExportError {
            throw error
        } catch {
            throw AudioExportError.writeFailed(error.localizedDescription)
        }
    }

    private func copySamples(_ samples: [Float], into buffer: AVAudioPCMBuffer) {
        if let channelData = buffer.floatChannelData, !samples.isEmpty {
            samples.withUnsafeBufferPointer { source in
                if let base = source.baseAddress {
                    channelData[0].update(from: base, count: samples.count)
                }
            }
        }
    }

    private func samples(from buffer: AVAudioPCMBuffer) -> [Float] {
        let count = Int(buffer.frameLength)
        guard count > 0, let channelData = buffer.floatChannelData else {
            return []
        }
        return Array(UnsafeBufferPointer(start: channelData[0], count: count))
    }
}
