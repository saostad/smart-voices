import Foundation
import NaturalLanguage

/// A chunk of speakable text with its trailing pause.
public struct SpeechChunk: Sendable, Equatable {
    public var text: String
    public var pauseAfter: SpeechPause

    public init(text: String, pauseAfter: SpeechPause) {
        self.text = text
        self.pauseAfter = pauseAfter
    }
}

/// Splits segments into sentence-bounded chunks for TTS.
public struct SpeechChunker: Sendable {
    private let maxCharacters: Int
    private let firstChunkMaxCharacters: Int

    public init(maxCharacters: Int = 300, firstChunkMaxCharacters: Int = 120) {
        self.maxCharacters = maxCharacters
        self.firstChunkMaxCharacters = firstChunkMaxCharacters
    }

    /// Split segments into chunks.
    public func chunks(from segments: [SpeechSegment]) -> [SpeechChunk] {
        if segments.isEmpty {
            return []
        }
        var result: [SpeechChunk] = []
        var isFirstChunk = true
        for segment in segments {
            let trimmed = segment.text.trimmingCharacters(in: .whitespacesAndNewlines)
            if trimmed.isEmpty {
                continue
            }
            var texts: [String] = []
            for sentence in splitSentences(trimmed) {
                texts.append(contentsOf: splitSentence(sentence, isFirst: &isFirstChunk))
            }
            for (index, text) in texts.enumerated() {
                let pause: SpeechPause = (index == texts.count - 1) ? segment.pause : .none
                result.append(SpeechChunk(text: text, pauseAfter: pause))
            }
        }
        return result
    }

    private func splitSentences(_ text: String) -> [String] {
        let tokenizer = NLTokenizer(unit: .sentence)
        tokenizer.string = text
        var sentences: [String] = []
        tokenizer.enumerateTokens(in: text.startIndex..<text.endIndex) { range, _ in
            let sentence = String(text[range]).trimmingCharacters(in: .whitespacesAndNewlines)
            if !sentence.isEmpty {
                sentences.append(sentence)
            }
            return true
        }
        if sentences.isEmpty {
            let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
            if !trimmed.isEmpty {
                return [trimmed]
            }
        }
        return sentences
    }

    private func splitSentence(_ sentence: String, isFirst: inout Bool) -> [String] {
        var remaining = sentence.trimmingCharacters(in: .whitespacesAndNewlines)
        if remaining.isEmpty {
            return []
        }
        var parts: [String] = []
        while !remaining.isEmpty {
            remaining = remaining.trimmingCharacters(in: .whitespacesAndNewlines)
            if remaining.isEmpty {
                break
            }
            let limit = isFirst ? firstChunkMaxCharacters : maxCharacters
            if remaining.count <= limit {
                parts.append(remaining)
                isFirst = false
                break
            }
            if let (first, rest) = splitBeforeLimit(remaining, limit: limit) {
                let cleaned = first.trimmingCharacters(in: .whitespacesAndNewlines)
                if !cleaned.isEmpty {
                    parts.append(cleaned)
                }
                isFirst = false
                remaining = rest
            } else if let (first, rest) = splitAfterLimit(remaining, limit: limit) {
                let cleaned = first.trimmingCharacters(in: .whitespacesAndNewlines)
                if !cleaned.isEmpty {
                    parts.append(cleaned)
                }
                isFirst = false
                remaining = rest
            } else {
                // Single long word: emit whole to avoid mid-word split.
                parts.append(remaining)
                isFirst = false
                break
            }
        }
        return parts
    }

    private func splitBeforeLimit(_ text: String, limit: Int) -> (String, String)? {
        if limit <= 0 || text.count <= limit {
            return nil
        }
        let prefixEnd = text.index(text.startIndex, offsetBy: limit)
        let prefix = text[..<prefixEnd]
        for separator in ["; ", ", ", " "] {
            if let range = prefix.range(of: separator, options: .backwards) {
                if separator == " " {
                    let first = String(text[..<range.lowerBound])
                    let rest = String(text[range.upperBound...])
                    if !first.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                        return (first, rest)
                    }
                } else {
                    // Keep ';' or ',' in the first part, drop the space.
                    let first = String(text[..<range.upperBound].dropLast())
                    let rest = String(text[range.upperBound...])
                    if !first.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                        return (first, rest)
                    }
                }
            }
        }
        return nil
    }

    private func splitAfterLimit(_ text: String, limit: Int) -> (String, String)? {
        if limit < 0 || text.count <= limit {
            return nil
        }
        let startSearch = text.index(text.startIndex, offsetBy: limit)
        let suffix = text[startSearch...]
        for separator in ["; ", ", ", " "] {
            if let range = suffix.range(of: separator) {
                if separator == " " {
                    let first = String(text[..<range.lowerBound])
                    let rest = String(text[range.upperBound...])
                    return (first, rest)
                } else {
                    let first = String(text[..<range.upperBound].dropLast())
                    let rest = String(text[range.upperBound...])
                    return (first, rest)
                }
            }
        }
        return nil
    }
}
