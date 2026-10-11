import Foundation
import Testing
@testable import SmartVoicesCore

private struct PrefixRewriter: ConversationRewriter {
    func availability() -> RewriterAvailability { .available }
    func rewrite(_ text: String) async throws -> String { "Retold: \(text)" }
}

private struct UnavailableFakeRewriter: ConversationRewriter {
    func availability() -> RewriterAvailability { .unavailable(reason: "Model off") }
    func rewrite(_ text: String) async throws -> String { "unreachable" }
}

private struct ThrowingRewriter: ConversationRewriter {
    func availability() -> RewriterAvailability { .available }
    func rewrite(_ text: String) async throws -> String { throw FakeFailure() }
}

private struct EmptyRewriter: ConversationRewriter {
    func availability() -> RewriterAvailability { .available }
    func rewrite(_ text: String) async throws -> String { "  " }
}

private struct SlowRewriter: ConversationRewriter, @unchecked Sendable {
    func availability() -> RewriterAvailability { .available }
    func rewrite(_ text: String) async throws -> String {
        try await Task.sleep(for: .milliseconds(300))
        return "Slow: \(text)"
    }
}

@Suite @MainActor
struct ConversationRewriteWiringTests {
    func waitForIdle(_ controller: PlaybackController) async {
        let start = ContinuousClock.now
        while controller.state != .idle {
            if ContinuousClock.now - start > .seconds(5) { break }
            try? await Task.sleep(for: .milliseconds(10))
        }
    }

    @Test func playsRewrittenText() async {
        let engine = FakeEngine()
        let output = FakeOutput()
        let controller = PlaybackController(engine: engine, output: output)
        controller.rewriter = PrefixRewriter()
        controller.play(text: "Hello there.")
        await waitForIdle(controller)
        #expect(controller.state == .idle)
        #expect(controller.lastNotice == nil)
        #expect(controller.chunks.map(\.text) == ["Retold: Hello there."])
        #expect(engine.synthesizeCalls.map(\.text) == ["Retold: Hello there."])
    }

    @Test func unavailableRewriterFallsBackWithNotice() async {
        let engine = FakeEngine()
        let output = FakeOutput()
        let controller = PlaybackController(engine: engine, output: output)
        controller.rewriter = UnavailableFakeRewriter()
        controller.play(text: "Hello there.")
        await waitForIdle(controller)
        #expect(controller.chunks.map(\.text) == ["Hello there."])
        #expect(engine.synthesizeCalls.map(\.text) == ["Hello there."])
        #expect(
            controller.lastNotice
                == "Conversational rewrite unavailable: Model off Reading as-is.")
    }

    @Test func throwingRewriterFallsBackWithNotice() async {
        let engine = FakeEngine()
        let output = FakeOutput()
        let controller = PlaybackController(engine: engine, output: output)
        controller.rewriter = ThrowingRewriter()
        controller.play(text: "Hello there.")
        await waitForIdle(controller)
        #expect(controller.chunks.map(\.text) == ["Hello there."])
        #expect(
            controller.lastNotice
                == "Conversational rewrite failed: boom. Reading as-is.")
    }

    @Test func emptyRewriteFallsBackWithNotice() async {
        let engine = FakeEngine()
        let output = FakeOutput()
        let controller = PlaybackController(engine: engine, output: output)
        controller.rewriter = EmptyRewriter()
        controller.play(text: "Hello there.")
        await waitForIdle(controller)
        #expect(controller.chunks.map(\.text) == ["Hello there."])
        #expect(
            controller.lastNotice
                == "Conversational rewrite returned no text. Reading as-is.")
    }

    @Test func staleRewriteIsCancelled() async {
        let engine = FakeEngine()
        let output = FakeOutput()
        let controller = PlaybackController(engine: engine, output: output)
        controller.rewriter = SlowRewriter()
        controller.play(text: "First text here.")
        controller.play(text: "Second text here.")
        await waitForIdle(controller)
        #expect(controller.chunks.map(\.text) == ["Slow: Second text here."])
        #expect(engine.synthesizeCalls.map(\.text) == ["Slow: Second text here."])
        #expect(controller.lastNotice == nil)
    }

    @Test func stopDuringRewritePlaysNothing() async {
        let engine = FakeEngine()
        let output = FakeOutput()
        let controller = PlaybackController(engine: engine, output: output)
        controller.rewriter = SlowRewriter()
        controller.play(text: "First text here.")
        controller.stop()
        // Give the cancelled rewrite time to (not) deliver.
        try? await Task.sleep(for: .milliseconds(500))
        #expect(controller.state == .idle)
        #expect(engine.synthesizeCalls.isEmpty)
    }

    @Test func rewriteHelperHonoursCancellation() async {
        let slow = SlowRewriter()
        let result = await ConversationalRewrite.rewrite(
            ["Hello."], with: slow, isCancelled: { true })
        #expect(result.text == nil)
        #expect(result.notice == nil)
    }

    @Test func attemptedRewriterReportsSupportReason() {
        let attempted = ConversationRewriteSupport.makeAttemptedRewriter()
        #expect(attempted.availability() == ConversationRewriteSupport.availability())
    }

    @Test func conversationalModeSettingRoundTrip() {
        let defaults = UserDefaults(suiteName: "sv-tests-\(UUID().uuidString)")!
        let settings = AppSettings(defaults: defaults)
        #expect(settings.conversationalMode == false)
        settings.conversationalMode = true
        #expect(AppSettings(defaults: defaults).conversationalMode == true)
    }
}
