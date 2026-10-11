import Testing
@testable import SmartVoicesCore

@Suite
struct WordTimingTests {
    @Test func startOfChunk() {
        #expect(WordTiming.wordIndex(in: "Hello world today", elapsed: 0, duration: 10) == 0)
    }

    @Test func middleOfChunk() {
        // Each word weighs 4 (3 chars + 1); boundaries at 4 and 8 of 12.
        #expect(WordTiming.wordIndex(in: "aaa bbb ccc", elapsed: 5, duration: 12) == 1)
        #expect(WordTiming.wordIndex(in: "aaa bbb ccc", elapsed: 2, duration: 12) == 0)
        #expect(WordTiming.wordIndex(in: "aaa bbb ccc", elapsed: 11, duration: 12) == 2)
    }

    @Test func endOfChunk() {
        #expect(WordTiming.wordIndex(in: "aaa bbb ccc", elapsed: 12, duration: 12) == 2)
    }

    @Test func emptyText() {
        #expect(WordTiming.wordIndex(in: "", elapsed: 0, duration: 10) == nil)
        #expect(WordTiming.wordIndex(in: "   ", elapsed: 0, duration: 10) == nil)
    }

    @Test func elapsedPastDuration() {
        #expect(WordTiming.wordIndex(in: "aaa bbb ccc", elapsed: 12.5, duration: 12) == nil)
    }

    @Test func invalidTiming() {
        #expect(WordTiming.wordIndex(in: "aaa bbb", elapsed: 1, duration: 0) == nil)
        #expect(WordTiming.wordIndex(in: "aaa bbb", elapsed: -1, duration: 10) == nil)
    }

    @Test func punctuationAddsWeight() {
        // "Hi!" weighs 4.5 (3 + 1 + 0.5), "Bye" weighs 4; total 8.5.
        // Elapsed 4.2 lands in the first word only because of the "!".
        #expect(WordTiming.wordIndex(in: "Hi! Bye", elapsed: 4.2, duration: 8.5) == 0)
        #expect(WordTiming.wordIndex(in: "Hi Bye", elapsed: 4.2, duration: 8) == 1)
    }
}
