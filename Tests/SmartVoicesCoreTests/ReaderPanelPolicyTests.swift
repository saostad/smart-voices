import Testing
@testable import SmartVoicesCore

@Suite
struct ReaderPanelPolicyTests {
    @Test func showsWhenActiveAndSettingOnAndNotDismissed() {
        #expect(
            ReaderPanelPolicy.shouldShow(
                isActive: true, showSetting: true, dismissedReadingID: nil,
                currentReadingID: 1))
    }

    @Test func hiddenWhenDismissedForSameReading() {
        #expect(
            !ReaderPanelPolicy.shouldShow(
                isActive: true, showSetting: true, dismissedReadingID: 1,
                currentReadingID: 1))
    }

    @Test func showsAgainForDifferentReading() {
        #expect(
            ReaderPanelPolicy.shouldShow(
                isActive: true, showSetting: true, dismissedReadingID: 1,
                currentReadingID: 2))
    }

    @Test func hiddenWhenIdle() {
        #expect(
            !ReaderPanelPolicy.shouldShow(
                isActive: false, showSetting: true, dismissedReadingID: nil,
                currentReadingID: 1))
    }

    @Test func hiddenWhenSettingOff() {
        #expect(
            !ReaderPanelPolicy.shouldShow(
                isActive: true, showSetting: false, dismissedReadingID: nil,
                currentReadingID: 1))
    }
}
