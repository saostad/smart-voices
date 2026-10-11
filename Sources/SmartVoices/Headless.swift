import Foundation
import SmartVoicesCore
import SmartVoicesKokoro

func writeStderr(_ message: String) {
    if let data = (message + "\n").data(using: .utf8) {
        FileHandle.standardError.write(data)
    }
}

func msBetween(_ start: ContinuousClock.Instant, _ end: ContinuousClock.Instant) -> Int {
    let duration = start.duration(to: end)
    return Int(Double(duration.components.seconds) * 1000 + Double(duration.components.attoseconds) / 1_000_000_000_000_000)
}

func makeHeadlessEngine(engineID: String) -> any SpeechEngine {
    if engineID == "kokoro" {
        return KokoroSpeechEngine()
    }
    return SystemSpeechEngine()
}

func runHeadlessAsync(text: String, engine: any SpeechEngine) async -> Int32 {
    let sanitizer = SpeechSanitizer()
    let chunker = SpeechChunker()
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
    for (index, chunk) in chunks.enumerated() {
        let synthStart = ContinuousClock.now
        let audio: SynthesizedAudio
        do {
            audio = try await engine.synthesize(chunk.text, voiceID: nil, speed: 1.0)
        } catch {
            writeStderr("SmartVoices: Couldn't synthesize speech with \(engine.displayName): \(error.localizedDescription)")
            return 1
        }
        let synthMs = msBetween(synthStart, ContinuousClock.now)
        let audioMs = audio.sampleRate > 0 ? Int(Double(audio.samples.count) / audio.sampleRate * 1000) : 0
        print("chunk \(index): synth \(synthMs) ms, audio \(audioMs) ms")
        if first {
            first = false
            print("first audio after \(msBetween(start, ContinuousClock.now)) ms")
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
