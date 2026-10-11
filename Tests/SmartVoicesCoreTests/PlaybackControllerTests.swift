import Foundation
import Testing
@testable import SmartVoicesCore

final class FakeEngine: SpeechEngine, @unchecked Sendable {
    let displayName = "Fake"
    var delay: Duration = .zero
    var errorToThrow: Error?
    private let lock = NSLock()
    private var calls: [(text: String, voiceID: String?, speed: Double)] = []
    private var current = 0
    private var maxSeen = 0

    var synthesizeCalls: [(text: String, voiceID: String?, speed: Double)] {
        lock.lock()
        defer { lock.unlock() }
        return calls
    }

    var maxConcurrent: Int {
        lock.lock()
        defer { lock.unlock() }
        return maxSeen
    }

    func prepare() async throws {}

    func voices() async -> [VoiceInfo] { [] }

    func synthesize(_ text: String, voiceID: String?, speed: Double) async throws -> SynthesizedAudio {
        let (wait, failure): (Duration, Error?) = lock.withLock {
            current += 1
            maxSeen = max(maxSeen, current)
            calls.append((text, voiceID, speed))
            return (delay, errorToThrow)
        }
        defer {
            lock.withLock { current -= 1 }
        }
        if wait > .zero {
            try await Task.sleep(for: wait)
        }
        if let failure {
            throw failure
        }
        return SynthesizedAudio(samples: [Float(text.utf8.first ?? 0)], sampleRate: 24000)
    }
}

final class FakeOutput: AudioOutput {
    var autoComplete = true
    var playCalls: [SynthesizedAudio] = []
    var completions: [() -> Void] = []
    var pauseCount = 0
    var resumeCount = 0
    var stopCount = 0

    func play(_ audio: SynthesizedAudio, completion: @escaping @Sendable () -> Void) throws {
        playCalls.append(audio)
        if autoComplete {
            completion()
        } else {
            completions.append(completion)
        }
    }

    func pause() { pauseCount += 1 }
    func resume() { resumeCount += 1 }

    func stop() {
        stopCount += 1
    }

    func completeNext() {
        if !completions.isEmpty {
            completions.removeFirst()()
        }
    }

    func completeAll() {
        let pending = completions
        completions.removeAll()
        for completion in pending {
            completion()
        }
    }
}

struct FakeFailure: Error, LocalizedError {
    var errorDescription: String? { "boom" }
}

@Suite @MainActor
struct PlaybackControllerTests {
    func waitForIdle(_ controller: PlaybackController, timeout: Duration = .seconds(3)) async {
        let start = ContinuousClock.now
        while controller.state != .idle {
            if ContinuousClock.now - start > timeout {
                break
            }
            try? await Task.sleep(for: .milliseconds(10))
        }
    }

    func waitForPlays(_ output: FakeOutput, count: Int, timeout: Duration = .seconds(3)) async {
        let start = ContinuousClock.now
        while output.playCalls.count < count {
            if ContinuousClock.now - start > timeout {
                break
            }
            try? await Task.sleep(for: .milliseconds(10))
        }
    }

    @Test func playsChunksInOrder() async {
        let engine = FakeEngine()
        let output = FakeOutput()
        let controller = PlaybackController(engine: engine, output: output)
        controller.play(text: "First sentence. Second sentence. Third sentence.")
        await waitForIdle(controller)
        #expect(controller.state == .idle)
        #expect(output.playCalls.count == 3)
        let played = output.playCalls.map { $0.samples.first ?? 0 }
        #expect(played == [Float(70), Float(83), Float(84)])
        let texts = engine.synthesizeCalls.map(\.text).sorted()
        #expect(texts == ["First sentence.", "Second sentence.", "Third sentence."])
        #expect(controller.firstAudioLatency != nil)
    }

    @Test func lookaheadNeverExceedsTwo() async {
        let engine = FakeEngine()
        engine.delay = .milliseconds(80)
        let output = FakeOutput()
        let controller = PlaybackController(engine: engine, output: output)
        controller.play(text: "One. Two. Three. Four. Five. Six.")
        await waitForIdle(controller)
        #expect(controller.state == .idle)
        #expect(engine.maxConcurrent <= 2)
    }

    @Test func stopCancelsAndIgnoresLateCompletions() async {
        let engine = FakeEngine()
        let output = FakeOutput()
        output.autoComplete = false
        let controller = PlaybackController(engine: engine, output: output)
        controller.play(text: "One. Two. Three.")
        await waitForPlays(output, count: 1)
        #expect(output.playCalls.count == 1)
        controller.stop()
        #expect(controller.state == .idle)
        output.completeAll()
        try? await Task.sleep(for: .milliseconds(100))
        #expect(output.playCalls.count == 1)
        #expect(controller.state == .idle)
        #expect(controller.currentChunkIndex == 0)
    }

    @Test func skipMovesIndex() async {
        let engine = FakeEngine()
        let output = FakeOutput()
        output.autoComplete = false
        let controller = PlaybackController(engine: engine, output: output)
        controller.play(text: "One. Two. Three.")
        await waitForPlays(output, count: 1)
        #expect(controller.currentChunkIndex == 0)
        controller.skipForward()
        await waitForPlays(output, count: 2)
        #expect(controller.currentChunkIndex == 1)
        #expect(output.playCalls.count == 2)
        controller.skipBack()
        await waitForPlays(output, count: 3)
        #expect(controller.currentChunkIndex == 0)
        controller.stop()
    }

    @Test func engineErrorSetsLastError() async {
        let engine = FakeEngine()
        engine.errorToThrow = FakeFailure()
        let output = FakeOutput()
        let controller = PlaybackController(engine: engine, output: output)
        controller.play(text: "Hello there.")
        await waitForIdle(controller)
        #expect(controller.state == .idle)
        #expect(controller.lastError == "Couldn't synthesize speech with Fake: boom")
        #expect(output.playCalls.isEmpty)
    }

    @Test func emptyTextError() {
        let engine = FakeEngine()
        let output = FakeOutput()
        let controller = PlaybackController(engine: engine, output: output)
        controller.play(text: "   ")
        #expect(controller.state == .idle)
        #expect(controller.lastError == "Nothing to read: the clipboard text was empty after removing formatting.")
        #expect(output.playCalls.isEmpty)
    }

    @Test func toggleFromIdleWithNilStaysIdle() {
        let engine = FakeEngine()
        let output = FakeOutput()
        let controller = PlaybackController(engine: engine, output: output)
        controller.togglePlayPause { nil }
        #expect(controller.state == .idle)
    }

    @Test func togglePlayPauseCycle() async {
        let engine = FakeEngine()
        let output = FakeOutput()
        output.autoComplete = false
        let controller = PlaybackController(engine: engine, output: output)
        controller.togglePlayPause { "Hello world." }
        #expect(controller.state == .preparing || controller.state == .playing)
        await waitForPlays(output, count: 1)
        #expect(controller.state == .playing)
        controller.togglePlayPause { nil }
        #expect(controller.state == .paused)
        controller.togglePlayPause { nil }
        #expect(controller.state == .playing)
        controller.stop()
    }

    @Test func sanitizerOptionsReachEngine() async {
        let engine = FakeEngine()
        let output = FakeOutput()
        let controller = PlaybackController(engine: engine, output: output)
        controller.sanitizerOptions = SanitizerOptions(
            codeBlocks: .skip, tables: .skip, bareURLs: .skip, maxTableRowsToRead: 15)
        controller.play(
            text: "Intro.\n\n```python\nprint(1)\n```\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\nOutro.")
        await waitForIdle(controller)
        #expect(controller.state == .idle)
        #expect(engine.synthesizeCalls.map(\.text) == ["Intro.", "Outro."])
    }

    @Test func toggleFromPreparingPauses() {
        let engine = FakeEngine()
        engine.delay = .milliseconds(200)
        let output = FakeOutput()
        output.autoComplete = false
        let controller = PlaybackController(engine: engine, output: output)
        controller.play(text: "Hello world. Second.")
        #expect(controller.state == .preparing)
        controller.togglePlayPause { nil }
        #expect(controller.state == .paused)
        controller.stop()
    }

    @Test func jumpToIndex() async {
        let engine = FakeEngine()
        let output = FakeOutput()
        output.autoComplete = false
        let controller = PlaybackController(engine: engine, output: output)
        controller.play(text: "One. Two. Three.")
        await waitForPlays(output, count: 1)
        controller.jump(to: 2)
        await waitForPlays(output, count: 2)
        #expect(controller.currentChunkIndex == 2)
        #expect(output.playCalls.count == 2)
        // Second play call is "Three." (stale completion of chunk 0 ignored).
        #expect(output.playCalls[1].samples.first == Float(84))
        output.completeAll()
        await waitForIdle(controller)
        #expect(controller.state == .idle)
    }

    @Test func jumpClampsIntoRange() async {
        let engine = FakeEngine()
        let output = FakeOutput()
        output.autoComplete = false
        let controller = PlaybackController(engine: engine, output: output)
        controller.play(text: "One. Two. Three.")
        await waitForPlays(output, count: 1)
        controller.jump(to: 99)
        await waitForPlays(output, count: 2)
        #expect(controller.currentChunkIndex == 2)
        controller.jump(to: -3)
        await waitForPlays(output, count: 3)
        #expect(controller.currentChunkIndex == 0)
        controller.stop()
    }

    @Test func jumpWhenIdleIsNoOp() {
        let engine = FakeEngine()
        let output = FakeOutput()
        let controller = PlaybackController(engine: engine, output: output)
        controller.jump(to: 1)
        #expect(controller.state == .idle)
        #expect(output.playCalls.isEmpty)
    }

    @Test func chunkProgressAcrossPauseResume() async {
        let engine = FakeEngine()
        let output = FakeOutput()
        output.autoComplete = false
        let controller = PlaybackController(engine: engine, output: output)
        #expect(controller.currentChunkStartedAt == nil)
        #expect(controller.currentChunkDuration == 0)
        controller.play(text: "Hello world.")
        await waitForPlays(output, count: 1)
        guard let started = controller.currentChunkStartedAt else {
            Issue.record("startedAt should be set once audio is handed to the output")
            return
        }
        #expect(controller.currentChunkDuration == 1.0 / 24000)
        let elapsedBeforePause = Date().timeIntervalSince(started)
        controller.pause()
        #expect(controller.currentChunkStartedAt == nil)
        #expect(controller.currentChunkDuration == 1.0 / 24000)
        try? await Task.sleep(for: .milliseconds(100))
        controller.resume()
        guard let resumed = controller.currentChunkStartedAt else {
            Issue.record("startedAt should be set again after resume")
            return
        }
        // The paused 100 ms must not count as spoken time.
        let elapsedAfterResume = Date().timeIntervalSince(resumed)
        #expect(abs(elapsedAfterResume - elapsedBeforePause) < 0.05)
        output.completeAll()
        await waitForIdle(controller)
        #expect(controller.currentChunkStartedAt == nil)
        #expect(controller.currentChunkDuration == 0)
    }

    @Test func chunkProgressClearedOnStop() async {
        let engine = FakeEngine()
        let output = FakeOutput()
        output.autoComplete = false
        let controller = PlaybackController(engine: engine, output: output)
        controller.play(text: "Hello world.")
        await waitForPlays(output, count: 1)
        #expect(controller.currentChunkStartedAt != nil)
        controller.stop()
        #expect(controller.currentChunkStartedAt == nil)
        #expect(controller.currentChunkDuration == 0)
    }
}
