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

/// Parse --engine (default kokoro). Exits 1 on error.
func headlessEngineID(args: [String]) -> String {
    var engineID = "kokoro"
    if let engineIndex = args.firstIndex(of: "--engine") {
        if engineIndex + 1 >= args.count {
            writeStderr("SmartVoices: missing argument for --engine")
            exit(1)
        }
        engineID = args[engineIndex + 1]
    }
    if engineID != "kokoro" && engineID != "system" {
        writeStderr("SmartVoices: unknown engine '\(engineID)'. Use kokoro or system.")
        exit(1)
    }
    return engineID
}

/// Parse --say/--say-file text. Exits 1 on error.
func headlessText(args: [String]) -> String {
    if let index = args.firstIndex(of: "--say") {
        if index + 1 >= args.count {
            writeStderr("SmartVoices: missing argument for --say")
            exit(1)
        }
        return args[index + 1]
    } else if let index = args.firstIndex(of: "--say-file") {
        if index + 1 >= args.count {
            writeStderr("SmartVoices: missing argument for --say-file")
            exit(1)
        }
        let path = args[index + 1]
        do {
            return try String(contentsOfFile: path, encoding: .utf8)
        } catch {
            writeStderr("SmartVoices: cannot read \(path): \(error.localizedDescription)")
            exit(1)
        }
    }
    writeStderr("SmartVoices: missing argument for --say")
    exit(1)
}

@MainActor
func runExportAsync(
    text: String, engine: any SpeechEngine, outputPath: String, conversational: Bool = false
) async -> Int32 {
    let exporter = AudioExporter()
    let rewriter: (any ConversationRewriter)? =
        conversational ? ConversationRewriteSupport.makeAttemptedRewriter() : nil
    do {
        let seconds = try await exporter.export(
            text: text, engine: engine, voiceID: nil, speed: 1.0,
            sanitizerOptions: SanitizerOptions(), rewriter: rewriter,
            to: URL(fileURLWithPath: outputPath))
        if let notice = exporter.lastNotice {
            writeStderr("SmartVoices: \(notice)")
        }
        print(String(format: "exported %.1f s to %@", seconds, outputPath))
        return 0
    } catch {
        writeStderr("SmartVoices: Couldn't export audio: \(error.localizedDescription)")
        return 1
    }
}

func runHeadlessAsync(
    text: String, engine: any SpeechEngine, conversational: Bool = false
) async -> Int32 {
    let sanitizer = SpeechSanitizer()
    let chunker = SpeechChunker()
    var text = text
    if conversational {
        let rewriter = ConversationRewriteSupport.makeAttemptedRewriter()
        let segments = sanitizer.sanitize(text)
        let result = await ConversationalRewrite.rewrite(
            RewriteBlocks.makeBlocks(from: segments), with: rewriter)
        if let rewritten = result.text {
            print("conversational rewrite applied")
            text = rewritten
        } else if let notice = result.notice {
            writeStderr("SmartVoices: \(notice)")
        } else {
            writeStderr("SmartVoices: conversational rewrite was cancelled.")
            return 1
        }
    }
    var engine = engine
    var voiceID: String?
    if engine is KokoroSpeechEngine,
        let detected = LanguageRouter.dominantLanguage(of: text),
        detected.code != "en"
    {
        guard let voice = SystemVoicePicker.bestVoice(forLanguage: detected.code) else {
            writeStderr("SmartVoices: \(LanguageRouting.missingVoiceMessage(for: detected.code))")
            return 1
        }
        engine = SystemSpeechEngine()
        voiceID = voice.id
    }
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
            audio = try await engine.synthesize(chunk.text, voiceID: voiceID, speed: 1.0)
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
