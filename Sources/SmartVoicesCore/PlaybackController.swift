import Foundation
import Observation
import os

/// Plays text aloud through a speech engine and audio output.
@MainActor @Observable
public final class PlaybackController {
    /// Playback state.
    public enum PlaybackState: Sendable, Equatable {
        case idle, preparing, playing, paused
    }

    public var state: PlaybackState = .idle
    public var currentChunkIndex: Int = 0
    public var chunks: [SpeechChunk] = []
    public var lastError: String?
    public var lastNotice: String?
    public var firstAudioLatency: Duration?
    public private(set) var currentChunkStartedAt: Date?
    public private(set) var currentChunkDuration: Double = 0
    public var speed: Double = 1.0
    public var voiceID: String?
    public var engine: any SpeechEngine

    public var sanitizerOptions: SanitizerOptions

    /// Pronunciations applied to chunk text right before synthesis.
    /// Display text (chunks) keeps the original spelling.
    public var pronunciationDictionary = PronunciationDictionary()

    /// Optional conversational rewriter. When set, text is retold in a spoken
    /// style before chunking; the display chunks show what is read.
    public var rewriter: (any ConversationRewriter)?

    private let output: any AudioOutput
    private let chunker: SpeechChunker
    private let logger = Logger(subsystem: "dev.smartvoices", category: "playback")

    private var generation = 0
    private var playbackTask: Task<Void, Never>?
    private var pausedElapsed: Double?
    private let pendingBox = PendingBox()
    private var activeEngineOverride: (any SpeechEngine)?
    private var activeVoiceOverride: String?

    public init(
        engine: any SpeechEngine,
        output: any AudioOutput,
        sanitizerOptions: SanitizerOptions = SanitizerOptions(),
        chunker: SpeechChunker = SpeechChunker()
    ) {
        self.engine = engine
        self.output = output
        self.sanitizerOptions = sanitizerOptions
        self.chunker = chunker
    }

    /// Play text from the start, stopping any current playback.
    /// The overrides apply to this play only; the stored engine/voice are unchanged.
    public func play(
        text: String, engineOverride: (any SpeechEngine)? = nil, voiceOverride: String? = nil
    ) {
        generation += 1
        playbackTask?.cancel()
        playbackTask = nil
        output.stop()
        resumePendingWithCancellation()
        activeEngineOverride = engineOverride
        activeVoiceOverride = voiceOverride
        lastError = nil
        lastNotice = nil
        firstAudioLatency = nil
        clearChunkProgress()
        let segments = SpeechSanitizer(options: sanitizerOptions).sanitize(text)
        let newChunks = chunker.chunks(from: segments)
        logger.notice("play requested: \(newChunks.count, privacy: .public) chunks")
        if newChunks.isEmpty {
            lastError = "Nothing to read: the clipboard text was empty after removing formatting."
            state = .idle
            chunks = []
            currentChunkIndex = 0
            return
        }
        let gen = generation
        let start = ContinuousClock.now
        if let rewriter {
            chunks = []
            currentChunkIndex = 0
            state = .preparing
            playbackTask = Task {
                await rewriteAndRun(
                    segments: segments, fallback: newChunks, rewriter: rewriter,
                    generation: gen, playStart: start)
            }
            return
        }
        chunks = newChunks
        currentChunkIndex = 0
        state = .preparing
        playbackTask = Task {
            await runLoop(from: 0, generation: gen, playStart: start)
        }
    }

    /// Rewrite first, then run the normal pipeline on the rewritten text.
    /// Any fallback keeps the original chunks and records a notice.
    private func rewriteAndRun(
        segments: [SpeechSegment], fallback: [SpeechChunk], rewriter: any ConversationRewriter,
        generation gen: Int, playStart: ContinuousClock.Instant
    ) async {
        let blocks = RewriteBlocks.makeBlocks(from: segments)
        // play/stop/skip cancel playbackTask, so task cancellation marks stale rewrites.
        // The generation check below stays as the backstop.
        let result = await ConversationalRewrite.rewrite(blocks, with: rewriter) {
            Task.isCancelled
        }
        if gen != generation { return }
        if let rewritten = result.text {
            let rewrittenChunks = chunker.chunks(
                from: SpeechSanitizer(options: sanitizerOptions).sanitize(rewritten))
            if rewrittenChunks.isEmpty {
                lastNotice = "Conversational rewrite returned no text. Reading as-is."
                logger.notice("conversational rewrite produced no chunks; reading as-is")
                chunks = fallback
            } else {
                logger.notice(
                    "conversational rewrite: \(rewrittenChunks.count, privacy: .public) chunks")
                chunks = rewrittenChunks
            }
        } else {
            if let notice = result.notice {
                lastNotice = notice
                logger.notice("\(notice, privacy: .public)")
            } else {
                return
            }
            chunks = fallback
        }
        currentChunkIndex = 0
        await runLoop(from: 0, generation: gen, playStart: playStart)
    }

    /// Pause playback.
    public func pause() {
        guard state == .playing || state == .preparing else { return }
        state = .paused
        if let startedAt = currentChunkStartedAt {
            pausedElapsed = Date().timeIntervalSince(startedAt)
            currentChunkStartedAt = nil
        }
        output.pause()
    }

    /// Resume playback.
    public func resume() {
        guard state == .paused else { return }
        state = .playing
        if let elapsed = pausedElapsed {
            currentChunkStartedAt = Date().addingTimeInterval(-elapsed)
            pausedElapsed = nil
        }
        output.resume()
    }

    /// Toggle between play/pause, starting playback with text() when idle.
    public func togglePlayPause(text: () -> String?) {
        switch state {
        case .idle:
            if let value = text() {
                play(text: value)
            }
        case .playing, .preparing:
            pause()
        case .paused:
            resume()
        }
    }

    /// Stop playback.
    public func stop() {
        generation += 1
        playbackTask?.cancel()
        playbackTask = nil
        output.stop()
        resumePendingWithCancellation()
        activeEngineOverride = nil
        activeVoiceOverride = nil
        state = .idle
        currentChunkIndex = 0
        clearChunkProgress()
    }

    /// Skip to the next chunk.
    public func skipForward() {
        jump(by: 1)
    }

    /// Skip to the previous chunk.
    public func skipBack() {
        jump(by: -1)
    }

    /// Jump to a chunk, clamping into range. No-op when idle.
    public func jump(to index: Int) {
        guard state != .idle, !chunks.isEmpty else { return }
        restart(at: min(max(index, 0), chunks.count - 1))
    }

    private func jump(by delta: Int) {
        guard state != .idle, !chunks.isEmpty else { return }
        let newIndex = currentChunkIndex + delta
        guard newIndex >= 0, newIndex < chunks.count else { return }
        restart(at: newIndex)
    }

    private func restart(at newIndex: Int) {
        generation += 1
        playbackTask?.cancel()
        playbackTask = nil
        output.stop()
        resumePendingWithCancellation()
        currentChunkIndex = newIndex
        clearChunkProgress()
        let gen = generation
        playbackTask = Task {
            await runLoop(from: newIndex, generation: gen, playStart: nil)
        }
    }

    private func clearChunkProgress() {
        currentChunkStartedAt = nil
        currentChunkDuration = 0
        pausedElapsed = nil
    }

    private func resumePendingWithCancellation() {
        if let pending = pendingBox.clear() {
            pending.resume(throwing: CancellationError())
        }
    }

    private func waitWhilePaused(generation gen: Int) async {
        while state == .paused, gen == generation {
            do {
                try await Task.sleep(for: .milliseconds(50))
            } catch {
                return
            }
        }
    }

    private func playAndWait(_ audio: SynthesizedAudio, generation gen: Int) async throws {
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            if gen != self.generation {
                continuation.resume(throwing: CancellationError())
                return
            }
            self.pendingBox.set(continuation, generation: gen)
            do {
                let box = self.pendingBox
                try self.output.play(audio) {
                    box.resumeIfMatch(generation: gen)
                }
            } catch {
                _ = self.pendingBox.clear()
                continuation.resume(throwing: error)
            }
        }
    }

    private func runLoop(from startIndex: Int, generation gen: Int, playStart: ContinuousClock.Instant?) async {
        let resolvedEngine = activeEngineOverride ?? engine
        let resolvedVoice = activeVoiceOverride ?? voiceID
        var cache: [Int: SynthesizedAudio] = [:]
        var synthTasks: [Int: Task<SynthesizedAudio, Error>] = [:]
        defer {
            for task in synthTasks.values {
                task.cancel()
            }
        }
        for index in startIndex..<chunks.count {
            if gen != generation { return }
            await waitWhilePaused(generation: gen)
            if gen != generation { return }
            let audio: SynthesizedAudio
            if let cached = cache[index] {
                audio = cached
            } else if let existing = synthTasks[index] {
                do {
                    audio = try await existing.value
                    cache[index] = audio
                    synthTasks.removeValue(forKey: index)
                } catch is CancellationError {
                    return
                } catch {
                    if gen != generation { return }
                    output.stop()
                    let message = "Couldn't synthesize speech with \(resolvedEngine.displayName): \(error.localizedDescription)"
                    logger.error("\(message, privacy: .public)")
                    lastError = message
                    state = .idle
                    clearChunkProgress()
                    if gen == generation { playbackTask = nil }
                    return
                }
            } else {
                let text = pronunciationDictionary.apply(to: chunks[index].text)
                let voice = resolvedVoice
                let spd = speed
                let eng = resolvedEngine
                do {
                    audio = try await eng.synthesize(text, voiceID: voice, speed: spd)
                    if gen != generation { return }
                    cache[index] = audio
                } catch is CancellationError {
                    return
                } catch {
                    if gen != generation { return }
                    output.stop()
                    let message = "Couldn't synthesize speech with \(resolvedEngine.displayName): \(error.localizedDescription)"
                    logger.error("\(message, privacy: .public)")
                    lastError = message
                    state = .idle
                    clearChunkProgress()
                    if gen == generation { playbackTask = nil }
                    return
                }
            }
            if gen != generation { return }
            for ahead in [index + 1, index + 2] {
                if ahead < chunks.count, cache[ahead] == nil, synthTasks[ahead] == nil {
                    let text = pronunciationDictionary.apply(to: chunks[ahead].text)
                    let voice = resolvedVoice
                    let spd = speed
                    let eng = resolvedEngine
                    synthTasks[ahead] = Task {
                        try await eng.synthesize(text, voiceID: voice, speed: spd)
                    }
                }
            }
            await waitWhilePaused(generation: gen)
            if gen != generation { return }
            currentChunkIndex = index
            if state == .preparing {
                state = .playing
            }
            if let start = playStart, index == startIndex, startIndex == 0 {
                let latency = start.duration(to: ContinuousClock.now)
                firstAudioLatency = latency
                let seconds = Double(latency.components.seconds)
                let ms = Int(seconds * 1000 + Double(latency.components.attoseconds) / 1_000_000_000_000_000)
                logger.notice("First audio latency: \(ms, privacy: .public) ms")
            }
            currentChunkStartedAt = Date()
            currentChunkDuration =
                audio.sampleRate > 0 ? Double(audio.samples.count) / audio.sampleRate : 0
            do {
                try await playAndWait(audio, generation: gen)
            } catch is CancellationError {
                return
            } catch {
                if gen != generation { return }
                output.stop()
                let message = "Couldn't play audio: \(error.localizedDescription)"
                logger.error("\(message, privacy: .public)")
                lastError = message
                state = .idle
                clearChunkProgress()
                if gen == generation { playbackTask = nil }
                return
            }
            if gen != generation { return }
            let pauseSeconds = chunks[index].pauseAfter.seconds / max(speed, 0.01)
            if pauseSeconds > 0 {
                do {
                    try await Task.sleep(for: .seconds(pauseSeconds))
                } catch {
                    return
                }
                if gen != generation { return }
            }
            cache.removeValue(forKey: index)
        }
        if gen == generation {
            state = .idle
            playbackTask = nil
            clearChunkProgress()
        }
    }
}

private final class PendingBox: @unchecked Sendable {
    private let lock = NSLock()
    private var continuation: CheckedContinuation<Void, Error>?
    private var generation = 0

    func set(_ continuation: CheckedContinuation<Void, Error>?, generation: Int) {
        lock.lock()
        defer { lock.unlock() }
        self.continuation = continuation
        self.generation = generation
    }

    func clear() -> CheckedContinuation<Void, Error>? {
        lock.lock()
        defer { lock.unlock() }
        let pending = continuation
        continuation = nil
        return pending
    }

    func resumeIfMatch(generation gen: Int) {
        lock.lock()
        guard generation == gen, let pending = continuation else {
            lock.unlock()
            return
        }
        continuation = nil
        lock.unlock()
        pending.resume()
    }
}
