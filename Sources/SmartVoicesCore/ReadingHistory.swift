import Foundation
import os

/// One item of recently read text.
public struct HistoryItem: Codable, Sendable, Equatable, Identifiable {
    public var id: UUID
    public var date: Date
    public var preview: String
    public var fullText: String

    public init(id: UUID, date: Date, preview: String, fullText: String) {
        self.id = id
        self.date = date
        self.preview = preview
        self.fullText = fullText
    }
}

/// The last 20 texts read, persisted as JSON. Used from the main actor only.
@MainActor
public final class ReadingHistory {
    public static let maxItems = 20

    public static var defaultFileURL: URL {
        if let base = FileManager.default.urls(
            for: .applicationSupportDirectory, in: .userDomainMask
        ).first {
            return base.appending(path: "Smart Voices/history.json")
        }
        return FileManager.default.homeDirectoryForCurrentUser.appending(
            path: "Library/Application Support/Smart Voices/history.json")
    }

    public private(set) var items: [HistoryItem] = []
    public private(set) var loadError: Error?

    private let fileURL: URL
    private let logger = Logger(subsystem: "dev.smartvoices", category: "history")

    public init(fileURL: URL? = nil) {
        self.fileURL = fileURL ?? Self.defaultFileURL
        load()
    }

    /// Record a text, moving duplicates to the top. Persists immediately.
    public func record(_ originalText: String) throws {
        let sanitized =
            SpeechSanitizer().sanitize(originalText).map(\.text).joined(separator: " ")
        let source = sanitized.isEmpty ? originalText : sanitized
        let preview = String(source.prefix(80))
        items.removeAll { $0.fullText == originalText }
        items.insert(
            HistoryItem(id: UUID(), date: Date(), preview: preview, fullText: originalText),
            at: 0)
        items = Array(items.prefix(Self.maxItems))
        try save()
    }

    /// Remove all items. Persists immediately.
    public func clear() throws {
        items = []
        try save()
    }

    private func load() {
        guard FileManager.default.fileExists(atPath: fileURL.path) else { return }
        do {
            let decoded = try JSONDecoder().decode(
                [HistoryItem].self, from: Data(contentsOf: fileURL))
            items = Array(decoded.prefix(Self.maxItems))
        } catch {
            loadError = error
            items = []
            logger.error("Couldn't load reading history: \(error.localizedDescription, privacy: .public)")
        }
    }

    private func save() throws {
        try FileManager.default.createDirectory(
            at: fileURL.deletingLastPathComponent(), withIntermediateDirectories: true)
        let data = try JSONEncoder().encode(items)
        try data.write(to: fileURL, options: .atomic)
    }
}
