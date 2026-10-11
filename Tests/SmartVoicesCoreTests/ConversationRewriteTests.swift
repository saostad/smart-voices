import Testing
@testable import SmartVoicesCore

private struct FakeRewriter: ConversationRewriter {
    var availabilityValue: RewriterAvailability = .available
    func availability() -> RewriterAvailability { availabilityValue }
    func rewrite(_ text: String) async throws -> String { "spoken: \(text)" }
}

@Suite
struct ConversationRewriteTests {
    @Test func emptySegmentsMakeNoBlocks() {
        #expect(RewriteBlocks.makeBlocks(from: []) == [])
    }

    @Test func singleSegmentIsOneBlock() {
        let segments = [SpeechSegment(text: "Hello there.", pause: .medium)]
        #expect(RewriteBlocks.makeBlocks(from: segments) == ["Hello there."])
    }

    @Test func blocksSplitAtSegmentBoundaries() {
        let segments = [
            SpeechSegment(text: "First sentence here.", pause: .medium),
            SpeechSegment(text: "Second sentence here.", pause: .medium),
            SpeechSegment(text: "Third.", pause: .medium),
        ]
        let blocks = RewriteBlocks.makeBlocks(from: segments, maxCharacters: 45)
        #expect(blocks == ["First sentence here. Second sentence here.", "Third."])
    }

    @Test func oversizedSegmentStaysWhole() {
        let long = String(repeating: "word ", count: 100).trimmingCharacters(in: .whitespaces)
        let segments = [
            SpeechSegment(text: "Short.", pause: .medium),
            SpeechSegment(text: long, pause: .medium),
            SpeechSegment(text: "After.", pause: .medium),
        ]
        let blocks = RewriteBlocks.makeBlocks(from: segments, maxCharacters: 50)
        #expect(blocks.count == 3)
        #expect(blocks[1] == long)
    }

    @Test func emptySegmentTextIsSkipped() {
        let segments = [
            SpeechSegment(text: "   ", pause: .medium),
            SpeechSegment(text: "Kept.", pause: .medium),
        ]
        #expect(RewriteBlocks.makeBlocks(from: segments) == ["Kept."])
    }

    @Test func instructionsAskForSameLanguageSpokenStyle() {
        let instructions = ConversationalPrompt.instructions.lowercased()
        #expect(instructions.contains("same language"))
        #expect(instructions.contains("read aloud") || instructions.contains("spoken"))
        #expect(instructions.contains("fact"))
        #expect(ConversationalPrompt.prompt(for: "Hi.").contains("Hi."))
    }

    @Test func fakeRewriterConforms() async throws {
        let rewriter = FakeRewriter()
        #expect(rewriter.availability() == .available)
        #expect(try await rewriter.rewrite("Hi.") == "spoken: Hi.")
    }

    @Test func supportAvailabilityMatchesRewriter() {
        // On this Mac the support answer must agree with a fresh support call
        // (stable), and either be available or carry a non-empty reason.
        let first = ConversationRewriteSupport.availability()
        let second = ConversationRewriteSupport.availability()
        #expect(first == second)
        if case .unavailable(let reason) = first {
            #expect(!reason.isEmpty)
            #expect(ConversationRewriteSupport.makeRewriter() == nil)
        } else {
            #expect(ConversationRewriteSupport.makeRewriter() != nil)
        }
    }
}
