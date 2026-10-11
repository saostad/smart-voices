import Foundation
import PDFKit

/// Errors from loading document files.
public enum DocumentTextError: Error, Sendable, Equatable {
    case unsupportedType(String)
    case unreadablePDF(String)
}

extension DocumentTextError: LocalizedError {
    public var errorDescription: String? {
        switch self {
        case .unsupportedType(let ext):
            return "unsupported file type '.\(ext)'."
        case .unreadablePDF(let name):
            return "couldn't parse \(name) as a PDF."
        }
    }
}

/// Loads readable text from Markdown, text, RTF and PDF files.
public enum DocumentTextLoader {
    /// File extensions this loader accepts (lowercased, no dot).
    public static let supportedExtensions = ["md", "markdown", "txt", "rtf", "pdf"]

    public static func load(url: URL) throws -> String {
        switch url.pathExtension.lowercased() {
        case "md", "markdown", "txt":
            return try String(contentsOf: url, encoding: .utf8)
        case "rtf":
            let data = try Data(contentsOf: url)
            let attributed = try NSAttributedString(
                data: data,
                options: [.documentType: NSAttributedString.DocumentType.rtf],
                documentAttributes: nil)
            return attributed.string
        case "pdf":
            guard let document = PDFDocument(url: url) else {
                throw DocumentTextError.unreadablePDF(url.lastPathComponent)
            }
            return document.string ?? ""
        case let ext:
            throw DocumentTextError.unsupportedType(ext)
        }
    }
}
