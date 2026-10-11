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
    }

    @Test func roundTrip() {
        let defaults = makeDefaults()
        let settings = AppSettings(defaults: defaults)
        settings.engineID = "system"
        settings.kokoroVoiceID = "am_adam"
        settings.systemVoiceID = "com.apple.voice.premium.en-US.Ava"
        settings.speed = 1.5
        settings.watchClipboard = false
        let reloaded = AppSettings(defaults: defaults)
        #expect(reloaded.engineID == "system")
        #expect(reloaded.kokoroVoiceID == "am_adam")
        #expect(reloaded.systemVoiceID == "com.apple.voice.premium.en-US.Ava")
        #expect(reloaded.speed == 1.5)
        #expect(reloaded.watchClipboard == false)
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
}
