import Testing
@testable import SmartVoicesCore

@Suite
struct KokoroVoiceCatalogTests {
    @Test func americanFemale() {
        #expect(KokoroVoiceCatalog.displayName(for: "af_heart") == "Heart — American female")
    }

    @Test func britishMale() {
        #expect(KokoroVoiceCatalog.displayName(for: "bm_george") == "George — British male")
    }

    @Test func otherAccents() {
        #expect(KokoroVoiceCatalog.displayName(for: "am_adam") == "Adam — American male")
        #expect(KokoroVoiceCatalog.displayName(for: "bf_emma") == "Emma — British female")
    }

    @Test func invalidIDs() {
        #expect(KokoroVoiceCatalog.displayName(for: "xx") == nil)
        #expect(KokoroVoiceCatalog.displayName(for: "zz_foo") == nil)
        #expect(KokoroVoiceCatalog.displayName(for: "afheart") == nil)
        #expect(KokoroVoiceCatalog.displayName(for: "af_") == nil)
    }
}
