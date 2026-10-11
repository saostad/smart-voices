import Foundation
import Testing
@testable import SmartVoicesCore

@Suite @MainActor
struct AppSettingsTests {
    private func makeDefaults() -> UserDefaults {
        UserDefaults(suiteName: "sv-tests-\(UUID().uuidString)")!
    }

    @Test func defaults() {
        let settings = AppSettings(defaults: makeDefaults())
        #expect(settings.engineID == "kokoro")
        #expect(settings.kokoroVoiceID == "af_heart")
        #expect(settings.systemVoiceID == nil)
        #expect(settings.speed == 1.0)
        #expect(settings.watchClipboard == true)
        #expect(settings.codeBlocks == "announce")
        #expect(settings.tables == "readRows")
        #expect(settings.bareURLs == "domainOnly")
        #expect(settings.maxTableRowsToRead == 15)
        #expect(settings.minimumReadyLength == 2)
        #expect(settings.ignoredAppBundleIDs == AppSettings.defaultIgnoredAppBundleIDs)
        #expect(settings.sanitizerOptions == SanitizerOptions())
    }

    @Test func roundTrip() {
        let defaults = makeDefaults()
        let settings = AppSettings(defaults: defaults)
        settings.engineID = "system"
        settings.kokoroVoiceID = "am_adam"
        settings.systemVoiceID = "com.apple.voice.premium.en-US.Ava"
        settings.speed = 1.5
        settings.watchClipboard = false
        settings.codeBlocks = "skip"
        settings.tables = "announce"
        settings.bareURLs = "skip"
        settings.maxTableRowsToRead = 5
        settings.minimumReadyLength = 10
        settings.ignoredAppBundleIDs = ["com.example.app"]
        let reloaded = AppSettings(defaults: defaults)
        #expect(reloaded.engineID == "system")
        #expect(reloaded.kokoroVoiceID == "am_adam")
        #expect(reloaded.systemVoiceID == "com.apple.voice.premium.en-US.Ava")
        #expect(reloaded.speed == 1.5)
        #expect(reloaded.watchClipboard == false)
        #expect(reloaded.codeBlocks == "skip")
        #expect(reloaded.tables == "announce")
        #expect(reloaded.bareURLs == "skip")
        #expect(reloaded.maxTableRowsToRead == 5)
        #expect(reloaded.minimumReadyLength == 10)
        #expect(reloaded.ignoredAppBundleIDs == ["com.example.app"])
        #expect(
            reloaded.sanitizerOptions
                == SanitizerOptions(
                    codeBlocks: .skip, tables: .announce, bareURLs: .skip, maxTableRowsToRead: 5))
        settings.systemVoiceID = nil
        #expect(AppSettings(defaults: defaults).systemVoiceID == nil)
    }

    @Test func clamping() {
        let defaults = makeDefaults()
        let settings = AppSettings(defaults: defaults)
        settings.speed = 5.0
        #expect(settings.speed == 3.0)
        settings.speed = 0.1
        #expect(settings.speed == 0.5)
        #expect(AppSettings(defaults: defaults).speed == 0.5)
        defaults.set(10.0, forKey: "sv.speed")
        #expect(AppSettings(defaults: defaults).speed == 3.0)
        settings.maxTableRowsToRead = 0
        #expect(settings.maxTableRowsToRead == 1)
        settings.maxTableRowsToRead = 500
        #expect(settings.maxTableRowsToRead == 100)
        settings.minimumReadyLength = 0
        #expect(settings.minimumReadyLength == 1)
        settings.minimumReadyLength = 5000
        #expect(settings.minimumReadyLength == 1000)
        defaults.set(500, forKey: "sv.maxTableRowsToRead")
        #expect(AppSettings(defaults: defaults).maxTableRowsToRead == 100)
        defaults.set(-3, forKey: "sv.minimumReadyLength")
        #expect(AppSettings(defaults: defaults).minimumReadyLength == 1)
    }

    @Test func invalidEngineReset() {
        let defaults = makeDefaults()
        defaults.set("elevenlabs", forKey: "sv.engineID")
        let settings = AppSettings(defaults: defaults)
        #expect(settings.engineID == "kokoro")
        #expect(defaults.string(forKey: "sv.engineID") == "kokoro")
        settings.engineID = "bogus"
        #expect(settings.engineID == "kokoro")
    }

    @Test func hotKeysRoundTrip() {
        let defaults = makeDefaults()
        let settings = AppSettings(defaults: defaults)
        let custom = HotKey(keyCode: 15, modifiers: HotKey.optionKey | HotKey.cmdKey)
        settings.hotKeys[.playPause] = custom
        settings.hotKeys.removeValue(forKey: .stop)
        let reloaded = AppSettings(defaults: defaults)
        #expect(reloaded.hotKeys[.playPause] == custom)
        #expect(reloaded.hotKeys[.stop] == nil)
        #expect(reloaded.hotKeys[.nextSentence] == AppSettings.defaultHotKeys[.nextSentence])
        defaults.set(Data("not json".utf8), forKey: "sv.hotKeys")
        #expect(AppSettings(defaults: defaults).hotKeys == AppSettings.defaultHotKeys)
    }

    @Test func invalidReadingOptionsReset() {
        let defaults = makeDefaults()
        defaults.set("shout", forKey: "sv.codeBlocks")
        defaults.set("chart", forKey: "sv.tables")
        defaults.set("full", forKey: "sv.bareURLs")
        let settings = AppSettings(defaults: defaults)
        #expect(settings.codeBlocks == "announce")
        #expect(settings.tables == "readRows")
        #expect(settings.bareURLs == "domainOnly")
        #expect(defaults.string(forKey: "sv.codeBlocks") == "announce")
        settings.codeBlocks = "bogus"
        settings.tables = "bogus"
        settings.bareURLs = "bogus"
        #expect(settings.codeBlocks == "announce")
        #expect(settings.tables == "readRows")
        #expect(settings.bareURLs == "domainOnly")
        #expect(settings.sanitizerOptions == SanitizerOptions())
    }
}
