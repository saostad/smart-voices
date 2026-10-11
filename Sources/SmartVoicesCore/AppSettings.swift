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
    public static let defaultCodeBlocks = "announce"
    public static let defaultTables = "readRows"
    public static let defaultBareURLs = "domainOnly"
    public static let defaultMaxTableRowsToRead = 15
    public static let minMaxTableRowsToRead = 1
    public static let maxMaxTableRowsToRead = 100
    public static let defaultMinimumReadyLength = 2
    public static let minMinimumReadyLength = 1
    public static let maxMinimumReadyLength = 1000
    public static let defaultIgnoredAppBundleIDs = [
        "com.1password.1password",
        "com.agilebits.onepassword7",
        "com.bitwarden.desktop",
        "com.apple.keychainaccess",
        "com.apple.Passwords",
    ]
    public static let defaultHotKeys: [HotKeyAction: HotKey] = [
        .playPause: HotKey(keyCode: 49, modifiers: HotKey.controlKey | HotKey.optionKey),
        .stop: HotKey(keyCode: 47, modifiers: HotKey.controlKey | HotKey.optionKey),
        .nextSentence: HotKey(keyCode: 124, modifiers: HotKey.controlKey | HotKey.optionKey),
        .previousSentence: HotKey(keyCode: 123, modifiers: HotKey.controlKey | HotKey.optionKey),
    ]

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

    @ObservationIgnored private var _codeBlocks: String
    public var codeBlocks: String {
        get {
            access(keyPath: \.codeBlocks)
            return _codeBlocks
        }
        set {
            let value = Self.validatedCodeBlocks(newValue) ?? Self.defaultCodeBlocks
            if value != newValue {
                logger.warning("Unknown code blocks option '\(newValue, privacy: .public)'; resetting to default.")
            }
            withMutation(keyPath: \.codeBlocks) { _codeBlocks = value }
            defaults.set(value, forKey: "sv.codeBlocks")
        }
    }

    @ObservationIgnored private var _tables: String
    public var tables: String {
        get {
            access(keyPath: \.tables)
            return _tables
        }
        set {
            let value = Self.validatedTables(newValue) ?? Self.defaultTables
            if value != newValue {
                logger.warning("Unknown tables option '\(newValue, privacy: .public)'; resetting to default.")
            }
            withMutation(keyPath: \.tables) { _tables = value }
            defaults.set(value, forKey: "sv.tables")
        }
    }

    @ObservationIgnored private var _bareURLs: String
    public var bareURLs: String {
        get {
            access(keyPath: \.bareURLs)
            return _bareURLs
        }
        set {
            let value = Self.validatedBareURLs(newValue) ?? Self.defaultBareURLs
            if value != newValue {
                logger.warning("Unknown links option '\(newValue, privacy: .public)'; resetting to default.")
            }
            withMutation(keyPath: \.bareURLs) { _bareURLs = value }
            defaults.set(value, forKey: "sv.bareURLs")
        }
    }

    @ObservationIgnored private var _maxTableRowsToRead: Int
    public var maxTableRowsToRead: Int {
        get {
            access(keyPath: \.maxTableRowsToRead)
            return _maxTableRowsToRead
        }
        set {
            let clamped = min(Self.maxMaxTableRowsToRead, max(Self.minMaxTableRowsToRead, newValue))
            withMutation(keyPath: \.maxTableRowsToRead) { _maxTableRowsToRead = clamped }
            defaults.set(clamped, forKey: "sv.maxTableRowsToRead")
        }
    }

    @ObservationIgnored private var _minimumReadyLength: Int
    public var minimumReadyLength: Int {
        get {
            access(keyPath: \.minimumReadyLength)
            return _minimumReadyLength
        }
        set {
            let clamped = min(Self.maxMinimumReadyLength, max(Self.minMinimumReadyLength, newValue))
            withMutation(keyPath: \.minimumReadyLength) { _minimumReadyLength = clamped }
            defaults.set(clamped, forKey: "sv.minimumReadyLength")
        }
    }

    @ObservationIgnored private var _ignoredAppBundleIDs: [String]
    public var ignoredAppBundleIDs: [String] {
        get {
            access(keyPath: \.ignoredAppBundleIDs)
            return _ignoredAppBundleIDs
        }
        set {
            withMutation(keyPath: \.ignoredAppBundleIDs) { _ignoredAppBundleIDs = newValue }
            defaults.set(newValue, forKey: "sv.ignoredAppBundleIDs")
        }
    }

    @ObservationIgnored private var _keepHistory: Bool
    public var keepHistory: Bool {
        get {
            access(keyPath: \.keepHistory)
            return _keepHistory
        }
        set {
            withMutation(keyPath: \.keepHistory) { _keepHistory = newValue }
            defaults.set(newValue, forKey: "sv.keepHistory")
        }
    }

    @ObservationIgnored private var _showReaderPanel: Bool
    public var showReaderPanel: Bool {
        get {
            access(keyPath: \.showReaderPanel)
            return _showReaderPanel
        }
        set {
            withMutation(keyPath: \.showReaderPanel) { _showReaderPanel = newValue }
            defaults.set(newValue, forKey: "sv.showReaderPanel")
        }
    }

    @ObservationIgnored private var _hotKeys: [HotKeyAction: HotKey]
    /// Global shortcuts by action. A missing entry means disabled.
    public var hotKeys: [HotKeyAction: HotKey] {
        get {
            access(keyPath: \.hotKeys)
            return _hotKeys
        }
        set {
            withMutation(keyPath: \.hotKeys) { _hotKeys = newValue }
            do {
                let raw = Dictionary(uniqueKeysWithValues: newValue.map { ($0.rawValue, $1) })
                defaults.set(try JSONEncoder().encode(raw), forKey: "sv.hotKeys")
            } catch {
                logger.error("Couldn't save shortcuts: \(error.localizedDescription, privacy: .public)")
            }
        }
    }

    /// Sanitizer options matching the current reading settings.
    public var sanitizerOptions: SanitizerOptions {
        let code: SanitizerOptions.CodeBlocks =
            switch codeBlocks {
            case "skip": .skip
            case "readFull": .readFull
            default: .announce
            }
        let tables: SanitizerOptions.Tables =
            switch tables {
            case "announce": .announce
            case "skip": .skip
            default: .readRows
            }
        let urls: SanitizerOptions.BareURLs =
            switch bareURLs {
            case "skip": .skip
            default: .domainOnly
            }
        return SanitizerOptions(
            codeBlocks: code, tables: tables, bareURLs: urls,
            maxTableRowsToRead: maxTableRowsToRead)
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
        if defaults.object(forKey: "sv.showReaderPanel") != nil {
            self._showReaderPanel = defaults.bool(forKey: "sv.showReaderPanel")
        } else {
            self._showReaderPanel = true
        }
        if defaults.object(forKey: "sv.keepHistory") != nil {
            self._keepHistory = defaults.bool(forKey: "sv.keepHistory")
        } else {
            self._keepHistory = true
        }
        self._codeBlocks = Self.loadValidated(
            defaults: defaults, key: "sv.codeBlocks", validate: Self.validatedCodeBlocks,
            default: Self.defaultCodeBlocks, logger: logger)
        self._tables = Self.loadValidated(
            defaults: defaults, key: "sv.tables", validate: Self.validatedTables,
            default: Self.defaultTables, logger: logger)
        self._bareURLs = Self.loadValidated(
            defaults: defaults, key: "sv.bareURLs", validate: Self.validatedBareURLs,
            default: Self.defaultBareURLs, logger: logger)
        if let storedMaxRows = defaults.object(forKey: "sv.maxTableRowsToRead") as? Int {
            let clamped = min(Self.maxMaxTableRowsToRead, max(Self.minMaxTableRowsToRead, storedMaxRows))
            self._maxTableRowsToRead = clamped
            defaults.set(clamped, forKey: "sv.maxTableRowsToRead")
        } else {
            self._maxTableRowsToRead = Self.defaultMaxTableRowsToRead
        }
        if let storedMinLength = defaults.object(forKey: "sv.minimumReadyLength") as? Int {
            let clamped = min(Self.maxMinimumReadyLength, max(Self.minMinimumReadyLength, storedMinLength))
            self._minimumReadyLength = clamped
            defaults.set(clamped, forKey: "sv.minimumReadyLength")
        } else {
            self._minimumReadyLength = Self.defaultMinimumReadyLength
        }
        self._ignoredAppBundleIDs =
            defaults.stringArray(forKey: "sv.ignoredAppBundleIDs") ?? Self.defaultIgnoredAppBundleIDs
        if let storedHotKeys = defaults.data(forKey: "sv.hotKeys") {
            do {
                let raw = try JSONDecoder().decode([String: HotKey].self, from: storedHotKeys)
                var decoded: [HotKeyAction: HotKey] = [:]
                for (key, value) in raw {
                    if let action = HotKeyAction(rawValue: key) {
                        decoded[action] = value
                    }
                }
                self._hotKeys = decoded
            } catch {
                self._hotKeys = Self.defaultHotKeys
                do {
                    defaults.set(try JSONEncoder().encode(Self.defaultHotKeysRaw), forKey: "sv.hotKeys")
                } catch {
                    logger.error(
                        "Couldn't save default shortcuts: \(error.localizedDescription, privacy: .public)")
                }
                logger.warning(
                    "Couldn't read stored shortcuts; resetting to defaults: \(error.localizedDescription, privacy: .public)"
                )
            }
        } else {
            self._hotKeys = Self.defaultHotKeys
        }
    }

    private static var defaultHotKeysRaw: [String: HotKey] {
        Dictionary(uniqueKeysWithValues: defaultHotKeys.map { ($0.rawValue, $1) })
    }

    private static func validatedCodeBlocks(_ value: String) -> String? {
        switch value {
        case "announce", "skip", "readFull": return value
        default: return nil
        }
    }

    private static func validatedTables(_ value: String) -> String? {
        switch value {
        case "readRows", "announce", "skip": return value
        default: return nil
        }
    }

    private static func validatedBareURLs(_ value: String) -> String? {
        switch value {
        case "domainOnly", "skip": return value
        default: return nil
        }
    }

    private static func loadValidated(
        defaults: UserDefaults, key: String, validate: (String) -> String?,
        default defaultValue: String, logger: Logger
    ) -> String {
        guard let stored = defaults.string(forKey: key) else { return defaultValue }
        if let valid = validate(stored) { return valid }
        defaults.set(defaultValue, forKey: key)
        logger.warning("Unknown stored value '\(stored, privacy: .public)' for \(key, privacy: .public); resetting to default.")
        return defaultValue
    }
}
