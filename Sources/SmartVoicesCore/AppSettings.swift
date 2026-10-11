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

    // Custom accessors with manual observation: assigning to the property inside
    // its own didSet recurses forever once @Observable rewrites the storage.
    @ObservationIgnored private var _engineID: String
    public var engineID: String {
        get {
            access(keyPath: \.engineID)
            return _engineID
        }
        set {
            var value = newValue
            if value != "kokoro" && value != "system" {
                logger.warning("Unknown engine id '\(value, privacy: .public)'; resetting to default.")
                value = Self.defaultEngineID
            }
            withMutation(keyPath: \.engineID) { _engineID = value }
            defaults.set(value, forKey: "sv.engineID")
        }
    }

    @ObservationIgnored private var _kokoroVoiceID: String
    public var kokoroVoiceID: String {
        get {
            access(keyPath: \.kokoroVoiceID)
            return _kokoroVoiceID
        }
        set {
            withMutation(keyPath: \.kokoroVoiceID) { _kokoroVoiceID = newValue }
            defaults.set(newValue, forKey: "sv.kokoroVoiceID")
        }
    }

    @ObservationIgnored private var _systemVoiceID: String?
    public var systemVoiceID: String? {
        get {
            access(keyPath: \.systemVoiceID)
            return _systemVoiceID
        }
        set {
            withMutation(keyPath: \.systemVoiceID) { _systemVoiceID = newValue }
            defaults.set(newValue, forKey: "sv.systemVoiceID")
        }
    }

    @ObservationIgnored private var _speed: Double
    public var speed: Double {
        get {
            access(keyPath: \.speed)
            return _speed
        }
        set {
            let clamped = min(Self.maxSpeed, max(Self.minSpeed, newValue))
            withMutation(keyPath: \.speed) { _speed = clamped }
            defaults.set(clamped, forKey: "sv.speed")
        }
    }

    @ObservationIgnored private var _watchClipboard: Bool
    public var watchClipboard: Bool {
        get {
            access(keyPath: \.watchClipboard)
            return _watchClipboard
        }
        set {
            withMutation(keyPath: \.watchClipboard) { _watchClipboard = newValue }
            defaults.set(newValue, forKey: "sv.watchClipboard")
        }
    }

    private let defaults: UserDefaults
    private let logger = Logger(subsystem: "dev.smartvoices", category: "settings")

    public init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        let storedEngine = defaults.string(forKey: "sv.engineID") ?? Self.defaultEngineID
        if storedEngine == "kokoro" || storedEngine == "system" {
            self._engineID = storedEngine
        } else {
            self._engineID = Self.defaultEngineID
            defaults.set(Self.defaultEngineID, forKey: "sv.engineID")
            logger.warning("Unknown stored engine id '\(storedEngine, privacy: .public)'; resetting to default.")
        }
        self._kokoroVoiceID = defaults.string(forKey: "sv.kokoroVoiceID") ?? Self.defaultKokoroVoiceID
        self._systemVoiceID = defaults.string(forKey: "sv.systemVoiceID")
        if let storedSpeed = defaults.object(forKey: "sv.speed") as? Double {
            let clamped = min(Self.maxSpeed, max(Self.minSpeed, storedSpeed))
            self._speed = clamped
            defaults.set(clamped, forKey: "sv.speed")
        } else {
            self._speed = Self.defaultSpeed
        }
        if defaults.object(forKey: "sv.watchClipboard") != nil {
            self._watchClipboard = defaults.bool(forKey: "sv.watchClipboard")
        } else {
            self._watchClipboard = true
        }
    }
}
