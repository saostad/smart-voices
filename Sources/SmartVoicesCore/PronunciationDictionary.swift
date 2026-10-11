import Foundation

/// One pronunciation rule: how a written word should be spoken.
public struct PronunciationEntry: Codable, Sendable, Equatable, Identifiable {
    public var id: UUID
    public var written: String
    public var spoken: String
    public var caseSensitive: Bool
    public var wholeWord: Bool

    public init(
        id: UUID = UUID(), written: String, spoken: String,
        caseSensitive: Bool = false, wholeWord: Bool = true
    ) {
        self.id = id
        self.written = written
        self.spoken = spoken
        self.caseSensitive = caseSensitive
        self.wholeWord = wholeWord
    }

    /// Seed rules for common acronyms and jargon.
    public static let defaults: [PronunciationEntry] = [
        PronunciationEntry(written: "SQL", spoken: "sequel"),
        PronunciationEntry(written: "macOS", spoken: "mac O S"),
        PronunciationEntry(written: "iOS", spoken: "eye O S"),
        PronunciationEntry(written: "GitHub", spoken: "git hub"),
        PronunciationEntry(written: "LLM", spoken: "L L M"),
        PronunciationEntry(written: "TTS", spoken: "T T S"),
        PronunciationEntry(written: "API", spoken: "A P I"),
        PronunciationEntry(written: "UI", spoken: "U I"),
        PronunciationEntry(written: "JSON", spoken: "jason"),
        PronunciationEntry(written: "Kubernetes", spoken: "koo ber net ees"),
    ]
}

/// Applies pronunciation rules to text right before synthesis.
public struct PronunciationDictionary: Sendable, Equatable {
    public var entries: [PronunciationEntry]

    public init(entries: [PronunciationEntry] = []) {
        self.entries = entries
    }

    public func apply(to text: String) -> String {
        let rules = entries.filter { !$0.written.isEmpty }.sorted {
            $0.written.count > $1.written.count
        }
        guard !rules.isEmpty else { return text }
        var result = ""
        result.reserveCapacity(text.count)
        var index = text.startIndex
        while index < text.endIndex {
            var matched: PronunciationEntry?
            for rule in rules {
                if matches(rule, at: index, in: text) {
                    matched = rule
                    break
                }
            }
            if let matched {
                result += matched.spoken
                index = text.index(index, offsetBy: matched.written.count)
            } else {
                result.append(text[index])
                index = text.index(after: index)
            }
        }
        return result
    }

    private func matches(_ rule: PronunciationEntry, at index: String.Index, in text: String) -> Bool {
        guard let end = text.index(index, offsetBy: rule.written.count, limitedBy: text.endIndex),
            index < end
        else {
            return false
        }
        let candidate = text[index..<end]
        let equal =
            if rule.caseSensitive {
                candidate == rule.written[...]
            } else {
                candidate.lowercased() == rule.written.lowercased()
            }
        guard equal else { return false }
        guard rule.wholeWord else { return true }
        if index > text.startIndex {
            let before = text[text.index(before: index)]
            if before.isLetter || before.isNumber { return false }
        }
        if end < text.endIndex {
            let after = text[end]
            if after.isLetter || after.isNumber { return false }
        }
        return true
    }
}
