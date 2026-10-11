import Foundation
import Testing
@testable import SmartVoicesCore

@Suite @MainActor
struct SleepTimerTests {
    @Test func firesAfterDuration() async {
        let timer = SleepTimer()
        var fired = false
        timer.start(seconds: 1.1) { fired = true }
        #expect(timer.remaining != nil)
        let start = ContinuousClock.now
        while !fired {
            if ContinuousClock.now - start > .seconds(5) { break }
            try? await Task.sleep(for: .milliseconds(20))
        }
        #expect(fired)
        #expect(timer.remaining == nil)
    }

    @Test func cancelPreventsFiring() async {
        let timer = SleepTimer()
        var fired = false
        timer.start(seconds: 0.5) { fired = true }
        timer.cancel()
        #expect(timer.remaining == nil)
        try? await Task.sleep(for: .seconds(1))
        #expect(!fired)
    }

    @Test func restartReplacesPrevious() async {
        let timer = SleepTimer()
        var fires = 0
        timer.start(seconds: 0.5) { fires += 1 }
        timer.start(seconds: 30) { fires += 1 }
        try? await Task.sleep(for: .seconds(1))
        #expect(fires == 0)
        #expect(timer.remaining != nil)
        timer.cancel()
    }

    @Test func remainingCountsDown() async {
        let timer = SleepTimer()
        timer.start(seconds: 60) {}
        #expect(timer.remaining == 60)
        try? await Task.sleep(for: .seconds(2.2))
        let left = timer.remaining ?? -1
        #expect(left < 60 && left >= 55)
        timer.cancel()
        #expect(timer.remaining == nil)
    }

    @Test func formatCountdown() {
        #expect(SleepTimer.format(remaining: 872) == "14:32")
        #expect(SleepTimer.format(remaining: 60) == "1:00")
        #expect(SleepTimer.format(remaining: 5) == "0:05")
        #expect(SleepTimer.format(remaining: 0) == "0:00")
    }

    @Test func autoReadClipboardSettingRoundTrip() {
        let defaults = UserDefaults(suiteName: "sv-tests-\(UUID().uuidString)")!
        let settings = AppSettings(defaults: defaults)
        #expect(settings.autoReadClipboard == false)
        settings.autoReadClipboard = true
        #expect(AppSettings(defaults: defaults).autoReadClipboard == true)
    }
}
