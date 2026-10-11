import Foundation
import Observation
import os

/// Persisted app settings, written through to UserDefaults on every change.
@MainActor @Observable
public final class AppSettings {
    public static let defaultEngineID = "kokoro"
    public static let defaultKokoroVoiceID = "af_heart"
    public static let defaultSpeed = 1.0
    public static let minSpeed = 0.5
    public static let maxSpeed = 3.0

    public var engineID: String {
        didSet {
            if engineID != "kokoro" && engineID != "system" {
                logger.warning("Unknown engine id '\(self.engineID, privacy: .public)'; resetting to default.")
                engineID = Self.defaultEngineID
            }
            defaults.set(engineID, forKey: "sv.engineID")
        }
    }

    public var kokoroVoiceID: String {
        didSet { defaults.set(kokoroVoiceID, forKey: "sv.kokoroVoiceID") }
    }

    public var systemVoiceID: String? {
        didSet { defaults.set(systemVoiceID, forKey: "sv.systemVoiceID") }
    }

    public var speed: Double {
        didSet {
            speed = min(Self.maxSpeed, max(Self.minSpeed, speed))
            defaults.set(speed, forKey: "sv.speed")
        }
    }

    public var watchClipboard: Bool {
        didSet { defaults.set(watchClipboard, forKey: "sv.watchClipboard") }
    }

    private let defaults: UserDefaults
    private let logger = Logger(subsystem: "dev.smartvoices", category: "settings")

    public init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        let storedEngine = defaults.string(forKey: "sv.engineID") ?? Self.defaultEngineID
        if storedEngine == "kokoro" || storedEngine == "system" {
            self.engineID = storedEngine
        } else {
            self.engineID = Self.defaultEngineID
            defaults.set(Self.defaultEngineID, forKey: "sv.engineID")
            logger.warning("Unknown stored engine id '\(storedEngine, privacy: .public)'; resetting to default.")
        }
        self.kokoroVoiceID = defaults.string(forKey: "sv.kokoroVoiceID") ?? Self.defaultKokoroVoiceID
        self.systemVoiceID = defaults.string(forKey: "sv.systemVoiceID")
        if let storedSpeed = defaults.object(forKey: "sv.speed") as? Double {
            let clamped = min(Self.maxSpeed, max(Self.minSpeed, storedSpeed))
            self.speed = clamped
            defaults.set(clamped, forKey: "sv.speed")
        } else {
            self.speed = Self.defaultSpeed
        }
        if defaults.object(forKey: "sv.watchClipboard") != nil {
            self.watchClipboard = defaults.bool(forKey: "sv.watchClipboard")
        } else {
            self.watchClipboard = true
        }
    }
}
