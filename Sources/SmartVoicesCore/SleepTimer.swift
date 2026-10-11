import Foundation
import Observation

/// Wall-clock timer that stops playback after a chosen duration.
@MainActor @Observable
public final class SleepTimer {
    /// Seconds left, or nil when the timer is off.
    public private(set) var remaining: TimeInterval?

    private var timer: Timer?
    private var onFire: (() -> Void)?

    public init() {}

    /// Menu durations in minutes.
    public static let menuMinutes = [15, 30, 45, 60]

    /// Start (or restart) the timer. Seconds-based so tests stay fast.
    public func start(seconds: TimeInterval, onFire: @escaping () -> Void) {
        cancel()
        remaining = seconds
        self.onFire = onFire
        timer = Timer.scheduledTimer(withTimeInterval: 1.0, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.tick()
            }
        }
    }

    /// Turn the timer off.
    public func cancel() {
        timer?.invalidate()
        timer = nil
        onFire = nil
        remaining = nil
    }

    private func tick() {
        guard let left = remaining else { return }
        let next = left - 1
        if next <= 0 {
            let fire = onFire
            cancel()
            fire?()
        } else {
            remaining = next
        }
    }

    /// "14:32" style countdown for the menu title.
    public static func format(remaining: TimeInterval) -> String {
        let total = max(0, Int(remaining.rounded()))
        return "\(total / 60):\(String(format: "%02d", total % 60))"
    }
}
