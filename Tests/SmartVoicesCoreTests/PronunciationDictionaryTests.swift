import Foundation
import Testing
@testable import SmartVoicesCore

@Suite
struct PronunciationDictionaryTests {
    @Test func wholeWordSkipsSubstrings() {
        let dictionary = PronunciationDictionary(entries: [
            PronunciationEntry(written: "SQL", spoken: "sequel")
        ])
        #expect(dictionary.apply(to: "I love SQL.") == "I love sequel.")
        #expect(dictionary.apply(to: "MySQL is a database.") == "MySQL is a database.")
        #expect(dictionary.apply(to: "SQL, TTS and APIs.") == "sequel, TTS and APIs.")
    }

    @Test func substringEntryReplacesInsideWords() {
        let dictionary = PronunciationDictionary(entries: [
            PronunciationEntry(written: "sql", spoken: "sequel", wholeWord: false)
        ])
        #expect(dictionary.apply(to: "MySQL") == "Mysequel")
    }

    @Test func caseInsensitiveByDefault() {
        let dictionary = PronunciationDictionary(entries: [
            PronunciationEntry(written: "API", spoken: "A P I")
        ])
        #expect(dictionary.apply(to: "the api returns json") == "the A P I returns json")
    }

    @Test func caseSensitiveEntryKeepsCase() {
        let dictionary = PronunciationDictionary(entries: [
            PronunciationEntry(written: "API", spoken: "A P I", caseSensitive: true)
        ])
        #expect(dictionary.apply(to: "the api returns data") == "the api returns data")
        #expect(dictionary.apply(to: "the API returns data") == "the A P I returns data")
    }

    @Test func longestMatchWins() {
        let dictionary = PronunciationDictionary(entries: [
            PronunciationEntry(written: "GitHub", spoken: "git hub"),
            PronunciationEntry(written: "GitHub Actions", spoken: "git hub actions pipeline"),
        ])
        #expect(
            dictionary.apply(to: "GitHub Actions is great.") == "git hub actions pipeline is great.")
        #expect(dictionary.apply(to: "I use GitHub daily.") == "I use git hub daily.")
    }

    @Test func outputIsNotReprocessed() {
        let dictionary = PronunciationDictionary(entries: [
            PronunciationEntry(written: "x", spoken: "yy", wholeWord: false),
            PronunciationEntry(written: "yy", spoken: "z", wholeWord: false),
        ])
        #expect(dictionary.apply(to: "x") == "yy")
    }

    @Test func emptyWrittenIsIgnored() {
        let dictionary = PronunciationDictionary(entries: [
            PronunciationEntry(written: "", spoken: "oops")
        ])
        #expect(dictionary.apply(to: "hello") == "hello")
    }

    @Test func jsonRoundTrip() throws {
        let entries = PronunciationEntry.defaults
        let data = try JSONEncoder().encode(entries)
        #expect(try JSONDecoder().decode([PronunciationEntry].self, from: data) == entries)
    }

    @Test func defaultSeeds() {
        let seeds = Dictionary(
            uniqueKeysWithValues: PronunciationEntry.defaults.map { ($0.written, $0.spoken) })
        #expect(seeds.count == 10)
        #expect(seeds["SQL"] == "sequel")
        #expect(seeds["macOS"] == "mac O S")
        #expect(seeds["iOS"] == "eye O S")
        #expect(seeds["GitHub"] == "git hub")
        #expect(seeds["LLM"] == "L L M")
        #expect(seeds["TTS"] == "T T S")
        #expect(seeds["API"] == "A P I")
        #expect(seeds["UI"] == "U I")
        #expect(seeds["JSON"] == "jason")
        #expect(seeds["Kubernetes"] == "koo ber net ees")
    }
}

@Suite @MainActor
struct PronunciationPlaybackTests {
    @Test func engineHearsSpokenTextWhileChunksKeepOriginal() async {
        let engine = FakeEngine()
        let output = FakeOutput()
        let controller = PlaybackController(engine: engine, output: output)
        controller.pronunciationDictionary = PronunciationDictionary(entries: [
            PronunciationEntry(written: "SQL", spoken: "sequel")
        ])
        controller.play(text: "I love SQL.")
        let start = ContinuousClock.now
        while controller.state != .idle {
            if ContinuousClock.now - start > .seconds(3) { break }
            try? await Task.sleep(for: .milliseconds(10))
        }
        #expect(controller.state == .idle)
        #expect(engine.synthesizeCalls.map(\.text) == ["I love sequel."])
        #expect(controller.chunks.map(\.text) == ["I love SQL."])
    }

    @Test func pronunciationsSettingsRoundTrip() {
        let defaults = UserDefaults(suiteName: "sv-tests-\(UUID().uuidString)")!
        let settings = AppSettings(defaults: defaults)
        #expect(settings.pronunciations == AppSettings.defaultPronunciations)
        #expect(settings.pronunciations.count == 10)
        settings.pronunciations = [PronunciationEntry(written: "Grok", spoken: "grok")]
        let reloaded = AppSettings(defaults: defaults)
        #expect(reloaded.pronunciations == settings.pronunciations)
        defaults.set(Data("not json".utf8), forKey: "sv.pronunciations")
        #expect(AppSettings(defaults: defaults).pronunciations == AppSettings.defaultPronunciations)
    }
}
