import Foundation
import Testing
@testable import SmartVoicesCore

@Suite
struct LanguageRouterTests {
    @Test func detectsEnglish() {
        let detected = LanguageRouter.dominantLanguage(
            of: "This is a plain English sentence with plenty of ordinary words in it.")
        #expect(detected?.code == "en")
        #expect((detected?.confidence ?? 0) >= 0.6)
    }

    @Test func detectsGerman() {
        let detected = LanguageRouter.dominantLanguage(
            of: "Heute ist das Wetter sehr schön und wir gehen in den Park spazieren.")
        #expect(detected?.code == "de")
        #expect((detected?.confidence ?? 0) >= 0.6)
    }

    @Test func detectsPersian() {
        let detected = LanguageRouter.dominantLanguage(
            of: "سلام، امروز هوا خیلی خوب است و من به پارک می‌روم.")
        #expect(detected?.code == "fa")
        #expect((detected?.confidence ?? 0) >= 0.6)
    }

    @Test func arabicWithoutPersianLettersStaysArabic() {
        let detected = LanguageRouter.dominantLanguage(
            of: "مرحبا كيف حالك اليوم أتمنى أن تكون بخير وسعادة.")
        #expect(detected?.code == "ar")
    }

    @Test func shortTextIsNil() {
        #expect(LanguageRouter.dominantLanguage(of: "Hello world") == nil)
        #expect(LanguageRouter.dominantLanguage(of: "سلام") == nil)
    }

    @Test func textWithoutLettersIsNil() {
        #expect(LanguageRouter.dominantLanguage(of: "12345 !!! ??? 42.5") == nil)
        #expect(LanguageRouter.dominantLanguage(of: "   ") == nil)
    }

    @Test func missingVoiceMessage() {
        #expect(
            LanguageRouting.missingVoiceMessage(for: "fa")
                == "No Persian voice is installed. Add one in System Settings → Accessibility → Spoken Content → System Voice → Manage Voices."
        )
    }
}

@Suite @MainActor
struct EngineOverrideTests {
    func waitForIdle(_ controller: PlaybackController) async {
        let start = ContinuousClock.now
        while controller.state != .idle {
            if ContinuousClock.now - start > .seconds(3) { break }
            try? await Task.sleep(for: .milliseconds(10))
        }
    }

    @Test func overrideEngineIsUsedForThatPlayOnly() async {
        let stored = FakeEngine()
        let override = FakeEngine()
        let output = FakeOutput()
        let controller = PlaybackController(engine: stored, output: output)
        controller.voiceID = "stored-voice"
        controller.play(text: "Hello there.", engineOverride: override, voiceOverride: "alt-voice")
        await waitForIdle(controller)
        #expect(controller.state == .idle)
        #expect(override.synthesizeCalls.count == 1)
        #expect(override.synthesizeCalls.first?.voiceID == "alt-voice")
        #expect(stored.synthesizeCalls.isEmpty)
        controller.play(text: "Hello again.")
        await waitForIdle(controller)
        #expect(stored.synthesizeCalls.count == 1)
        #expect(stored.synthesizeCalls.first?.voiceID == "stored-voice")
        #expect(override.synthesizeCalls.count == 1)
    }

    @Test func autoSwitchLanguageSettingRoundTrip() {
        let defaults = UserDefaults(suiteName: "sv-tests-\(UUID().uuidString)")!
        let settings = AppSettings(defaults: defaults)
        #expect(settings.autoSwitchLanguage == true)
        settings.autoSwitchLanguage = false
        #expect(AppSettings(defaults: defaults).autoSwitchLanguage == false)
    }
}
