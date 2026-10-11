import Foundation

/// Errors from stitching audio chunks.
public enum AudioStitcherError: Error, Sendable, Equatable {
    case sampleRateMismatch(String)
    case mismatchedCounts(String)
}

extension AudioStitcherError: LocalizedError {
    public var errorDescription: String? {
        switch self {
        case .sampleRateMismatch(let message), .mismatchedCounts(let message):
            return message
        }
    }
}

/// Joins synthesized chunks with silence between them for export.
public enum AudioStitcher {
    /// Concatenate `audios`, appending `pauses[i]` seconds of silence after
    /// chunk `i`. All chunks must share a sample rate.
    public static func stitch(_ audios: [SynthesizedAudio], pauses: [Double]) throws
        -> SynthesizedAudio
    {
        guard audios.count == pauses.count else {
            throw AudioStitcherError.mismatchedCounts(
                "Cannot combine \(audios.count) audio chunks with \(pauses.count) pauses.")
        }
        guard let first = audios.first else {
            return SynthesizedAudio(samples: [], sampleRate: 24000)
        }
        for audio in audios.dropFirst() {
            if audio.sampleRate != first.sampleRate {
                throw AudioStitcherError.sampleRateMismatch(
                    "Cannot combine audio at different sample rates: \(first.sampleRate) Hz and \(audio.sampleRate) Hz."
                )
            }
        }
        var samples: [Float] = []
        samples.reserveCapacity(
            audios.reduce(0) { $0 + $1.samples.count }
                + Int(pauses.reduce(0, +) * first.sampleRate))
        for (audio, pause) in zip(audios, pauses) {
            samples.append(contentsOf: audio.samples)
            let silence = Int((max(pause, 0) * first.sampleRate).rounded())
            if silence > 0 {
                samples.append(contentsOf: repeatElement(0, count: silence))
            }
        }
        return SynthesizedAudio(samples: samples, sampleRate: first.sampleRate)
    }
}
