import Testing
@testable import SmartVoicesCore

@Suite struct SpeechChunkerTests {
    @Test func abbreviationsDoNotSplit() {
        let chunker = SpeechChunker()
        let result = chunker.chunks(from: [SpeechSegment(text: "Dr. Smith went home.", pause: .medium)])
        #expect(result.count == 1)
        #expect(result[0].text == "Dr. Smith went home.")
        #expect(result[0].pauseAfter == .medium)
    }

    @Test func decimalsDoNotSplit() {
        let chunker = SpeechChunker()
        let result = chunker.chunks(from: [SpeechSegment(text: "The value is 3.5 meters.", pause: .short)])
        #expect(result.count == 1)
        #expect(result[0].text == "The value is 3.5 meters.")
    }

    @Test func longSentenceSplitsAtSemicolon() {
        let chunker = SpeechChunker(maxCharacters: 30, firstChunkMaxCharacters: 30)
        let text = "Apples are red; oranges are orange; bananas are yellow."
        let result = chunker.chunks(from: [SpeechSegment(text: text, pause: .medium)])
        #expect(result.count == 3)
        #expect(result[0].text == "Apples are red;")
        #expect(result[1].text == "oranges are orange;")
        #expect(result[2].text == "bananas are yellow.")
        #expect(result.map(\.text).joined(separator: " ") == text)
    }

    @Test func longSentenceSplitsAtComma() {
        let chunker = SpeechChunker(maxCharacters: 20, firstChunkMaxCharacters: 20)
        let text = "One, two, three, four, five, six, seven."
        let result = chunker.chunks(from: [SpeechSegment(text: text, pause: .medium)])
        #expect(result.count == 3)
        #expect(result[0].text == "One, two, three,")
        #expect(result[1].text == "four, five, six,")
        #expect(result[2].text == "seven.")
        #expect(result.map(\.text).joined(separator: " ") == text)
    }

    @Test func longSentenceSplitsAtSpace() {
        let chunker = SpeechChunker(maxCharacters: 20, firstChunkMaxCharacters: 20)
        let text = "Alpha beta gamma delta epsilon zeta eta theta."
        let result = chunker.chunks(from: [SpeechSegment(text: text, pause: .medium)])
        #expect(result.count >= 2)
        for chunk in result {
            #expect(chunk.text.count <= 20)
            #expect(!chunk.text.isEmpty)
        }
        #expect(result.map(\.text).joined(separator: " ") == text)
    }

    @Test func firstChunkUsesFirstLimit() {
        let chunker = SpeechChunker(maxCharacters: 300, firstChunkMaxCharacters: 10)
        let result = chunker.chunks(from: [SpeechSegment(text: "Hello world, this is a test.", pause: .medium)])
        #expect(result.count == 2)
        #expect(result[0].text == "Hello")
        #expect(result[0].text.count <= 10)
        #expect(result[0].pauseAfter == .none)
        #expect(result[1].pauseAfter == .medium)
    }

    @Test func pauseCarryOver() {
        let chunker = SpeechChunker()
        let result = chunker.chunks(from: [
            SpeechSegment(text: "First.", pause: .short),
            SpeechSegment(text: "Second.", pause: .long),
        ])
        #expect(result == [
            SpeechChunk(text: "First.", pauseAfter: .short),
            SpeechChunk(text: "Second.", pauseAfter: .long),
        ])
        let multi = chunker.chunks(from: [SpeechSegment(text: "One. Two. Three.", pause: .medium)])
        #expect(multi.count == 3)
        #expect(multi[0].pauseAfter == .none)
        #expect(multi[1].pauseAfter == .none)
        #expect(multi[2].pauseAfter == .medium)
    }

    @Test func emptyInputReturnsEmpty() {
        let chunker = SpeechChunker()
        #expect(chunker.chunks(from: []) == [])
    }

    @Test func trimsAndSkipsEmpty() {
        let chunker = SpeechChunker()
        let result = chunker.chunks(from: [SpeechSegment(text: "  Hello.  ", pause: .medium)])
        #expect(result == [SpeechChunk(text: "Hello.", pauseAfter: .medium)])
    }
}
