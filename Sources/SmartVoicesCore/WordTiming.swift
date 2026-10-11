import Foundation

/// Estimates which word is being spoken from playback progress.
public enum WordTiming {
    /// Word index at `elapsed` seconds into `text` spoken over `duration`
    /// seconds, weighting each word by its character count plus 1 (plus 0.5
    /// per punctuation character). Returns nil for empty text, invalid
    /// timing, or `elapsed` past the end.
    public static func wordIndex(in text: String, elapsed: Double, duration: Double) -> Int? {
        let words = splitWords(text)
        if words.isEmpty {
            return nil
        }
        if !(elapsed >= 0) || !(duration > 0) || elapsed > duration {
            return nil
        }
        if elapsed == duration {
            return words.count - 1
        }
        let weights = words.map { weight(of: $0) }
        let total = weights.reduce(0, +)
        if total <= 0 {
            return 0
        }
        let position = (elapsed / duration) * total
        var cumulative = 0.0
        for (index, weight) in weights.enumerated() {
            cumulative += weight
            if position < cumulative {
                return index
            }
        }
        return words.count - 1
    }

    private static func splitWords(_ text: String) -> [String] {
        text.components(separatedBy: .whitespacesAndNewlines).filter { !$0.isEmpty }
    }

    private static func weight(of word: String) -> Double {
        let punctuation = word.unicodeScalars.filter {
            CharacterSet.punctuationCharacters.contains($0)
        }.count
        return Double(word.count + 1) + 0.5 * Double(punctuation)
    }
}
