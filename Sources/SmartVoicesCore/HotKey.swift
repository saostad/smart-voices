import Foundation

/// A global keyboard shortcut: a key code plus Carbon modifier bits.
public struct HotKey: Codable, Sendable, Equatable {
    /// Carbon modifier bits (HIToolbox), as used by RegisterEventHotKey.
    public static let cmdKey: UInt32 = 1 << 8
    public static let shiftKey: UInt32 = 1 << 9
    public static let optionKey: UInt32 = 1 << 11
    public static let controlKey: UInt32 = 1 << 12

    public var keyCode: UInt32
    public var modifiers: UInt32

    public init(keyCode: UInt32, modifiers: UInt32) {
        self.keyCode = keyCode
        self.modifiers = modifiers
    }

    /// Human-readable shortcut, e.g. "⌃⌥Space".
    public var displayString: String {
        var result = ""
        if modifiers & Self.controlKey != 0 { result += "⌃" }
        if modifiers & Self.optionKey != 0 { result += "⌥" }
        if modifiers & Self.shiftKey != 0 { result += "⇧" }
        if modifiers & Self.cmdKey != 0 { result += "⌘" }
        result += Self.keyName(for: keyCode)
        return result
    }

    private static func keyName(for keyCode: UInt32) -> String {
        switch keyCode {
        case 49: return "Space"
        case 36: return "Return"
        case 53: return "Escape"
        case 48: return "Tab"
        case 51: return "Delete"
        case 52: return "Enter"
        case 123: return "←"
        case 124: return "→"
        case 125: return "↓"
        case 126: return "↑"
        case 122: return "F1"
        case 120: return "F2"
        case 99: return "F3"
        case 118: return "F4"
        case 96: return "F5"
        case 97: return "F6"
        case 98: return "F7"
        case 100: return "F8"
        case 101: return "F9"
        case 109: return "F10"
        case 103: return "F11"
        case 111: return "F12"
        case 0: return "A"
        case 11: return "B"
        case 8: return "C"
        case 2: return "D"
        case 14: return "E"
        case 3: return "F"
        case 5: return "G"
        case 4: return "H"
        case 34: return "I"
        case 38: return "J"
        case 40: return "K"
        case 37: return "L"
        case 46: return "M"
        case 45: return "N"
        case 31: return "O"
        case 35: return "P"
        case 12: return "Q"
        case 15: return "R"
        case 1: return "S"
        case 17: return "T"
        case 32: return "U"
        case 9: return "V"
        case 13: return "W"
        case 7: return "X"
        case 16: return "Y"
        case 6: return "Z"
        case 29: return "0"
        case 18: return "1"
        case 19: return "2"
        case 20: return "3"
        case 21: return "4"
        case 23: return "5"
        case 22: return "6"
        case 26: return "7"
        case 28: return "8"
        case 25: return "9"
        case 47: return "."
        case 43: return ","
        case 44: return "/"
        case 41: return ";"
        case 39: return "'"
        case 33: return "["
        case 30: return "]"
        case 42: return "\\"
        case 27: return "-"
        case 24: return "="
        case 50: return "`"
        default: return "Key \(keyCode)"
        }
    }
}

/// Actions triggerable by global shortcuts.
public enum HotKeyAction: String, CaseIterable, Codable, Sendable {
    case playPause
    case stop
    case nextSentence
    case previousSentence
}
