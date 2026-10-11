import Foundation
import Testing
@testable import SmartVoicesCore

@Suite @MainActor
struct ReadingHistoryTests {
    private func makeURL() -> URL {
        FileManager.default.temporaryDirectory.appending(
            path: "sv-history-tests-\(UUID().uuidString)/history.json")
    }

    @Test func add() throws {
        let history = ReadingHistory(fileURL: makeURL())
        #expect(history.items.isEmpty)
        try history.record("# Hi **there**")
        #expect(history.items.count == 1)
        let item = try #require(history.items.first)
        #expect(item.preview == "Hi there.")
        #expect(item.fullText == "# Hi **there**")
        #expect(abs(item.date.timeIntervalSinceNow) < 5)
    }

    @Test func previewIsFirst80SanitizedChars() throws {
        let history = ReadingHistory(fileURL: makeURL())
        let long = "# Title\n\n" + String(repeating: "word ", count: 100)
        try history.record(long)
        let item = try #require(history.items.first)
        #expect(item.preview.count == 80)
        #expect(!item.preview.contains("#"))
        #expect(!item.preview.contains("**"))
    }

    @Test func dedupeMovesToTop() throws {
        let history = ReadingHistory(fileURL: makeURL())
        try history.record("First text.")
        try history.record("Second text.")
        try history.record("First text.")
        #expect(history.items.count == 2)
        #expect(history.items.map(\.fullText) == ["First text.", "Second text."])
    }

    @Test func capsAt20() throws {
        let history = ReadingHistory(fileURL: makeURL())
        for index in 1...21 {
            try history.record("Text number \(index).")
        }
        #expect(history.items.count == 20)
        #expect(history.items.first?.fullText == "Text number 21.")
        #expect(!history.items.map(\.fullText).contains("Text number 1."))
    }

    @Test func persistenceRoundTrip() throws {
        let url = makeURL()
        let history = ReadingHistory(fileURL: url)
        try history.record("Remember me.")
        try history.record("And me.")
        let reloaded = ReadingHistory(fileURL: url)
        #expect(reloaded.loadError == nil)
        #expect(reloaded.items == history.items)
        try reloaded.clear()
        #expect(reloaded.items.isEmpty)
        #expect(ReadingHistory(fileURL: url).items.isEmpty)
    }

    @Test func corruptFileIsErrorNotCrash() throws {
        let url = makeURL()
        try FileManager.default.createDirectory(
            at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Data("not json{{{".utf8).write(to: url)
        let history = ReadingHistory(fileURL: url)
        #expect(history.items.isEmpty)
        #expect(history.loadError != nil)
    }
}
