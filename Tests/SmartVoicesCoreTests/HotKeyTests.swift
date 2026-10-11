import Foundation
import Testing
@testable import SmartVoicesCore

@Suite @MainActor
struct HotKeyTests {
    @Test func displayString() {
        let ctrlOpt = HotKey.controlKey | HotKey.optionKey
        #expect(HotKey(keyCode: 49, modifiers: ctrlOpt).displayString == "⌃⌥Space")
        #expect(HotKey(keyCode: 47, modifiers: ctrlOpt).displayString == "⌃⌥.")
        #expect(HotKey(keyCode: 124, modifiers: ctrlOpt).displayString == "⌃⌥→")
        #expect(HotKey(keyCode: 123, modifiers: ctrlOpt).displayString == "⌃⌥←")
        #expect(HotKey(keyCode: 15, modifiers: HotKey.optionKey | HotKey.cmdKey).displayString == "⌥⌘R")
        #expect(HotKey(keyCode: 96, modifiers: HotKey.shiftKey | HotKey.cmdKey).displayString == "⇧⌘F5")
        #expect(HotKey(keyCode: 53, modifiers: HotKey.cmdKey).displayString == "⌘Escape")
        #expect(HotKey(keyCode: 36, modifiers: HotKey.controlKey).displayString == "⌃Return")
        #expect(HotKey(keyCode: 18, modifiers: HotKey.controlKey).displayString == "⌃1")
        #expect(HotKey(keyCode: 126, modifiers: 0).displayString == "↑")
        let all = HotKey.controlKey | HotKey.optionKey | HotKey.shiftKey | HotKey.cmdKey
        #expect(HotKey(keyCode: 7, modifiers: all).displayString == "⌃⌥⇧⌘X")
    }

    @Test func codableRoundTrip() throws {
        let key = HotKey(keyCode: 49, modifiers: HotKey.controlKey | HotKey.optionKey)
        let decoded = try JSONDecoder().decode(HotKey.self, from: JSONEncoder().encode(key))
        #expect(decoded == key)
    }

    @Test func defaults() {
        let defaults = AppSettings.defaultHotKeys
        #expect(defaults[.playPause]?.displayString == "⌃⌥Space")
        #expect(defaults[.stop]?.displayString == "⌃⌥.")
        #expect(defaults[.nextSentence]?.displayString == "⌃⌥→")
        #expect(defaults[.previousSentence]?.displayString == "⌃⌥←")
        let settings = AppSettings(
            defaults: UserDefaults(suiteName: "sv-tests-\(UUID().uuidString)")!)
        #expect(settings.hotKeys == AppSettings.defaultHotKeys)
    }
}
