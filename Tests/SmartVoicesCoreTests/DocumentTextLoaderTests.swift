import Foundation
import Testing
@testable import SmartVoicesCore

@Suite
struct DocumentTextLoaderTests {
    private func writeTempFile(named name: String, data: Data) throws -> URL {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("sv-test-\(UUID().uuidString)-\(name)")
        try data.write(to: url)
        return url
    }

    @Test func loadsMarkdownAndText() throws {
        let md = try writeTempFile(named: "note.md", data: Data("# Hello\n\nWorld".utf8))
        #expect(try DocumentTextLoader.load(url: md) == "# Hello\n\nWorld")
        let txt = try writeTempFile(named: "note.txt", data: Data("plain text".utf8))
        #expect(try DocumentTextLoader.load(url: txt) == "plain text")
    }

    @Test func loadsRTF() throws {
        let rtf = try writeTempFile(
            named: "note.rtf", data: Data(#"{\rtf1\ansi Hello RTF}"#.utf8))
        #expect(try DocumentTextLoader.load(url: rtf).contains("Hello RTF"))
    }

    @Test func loadsTinyPDF() throws {
        let pdf = try writeTempFile(named: "note.pdf", data: makePDFData(text: "Hello PDF"))
        let loaded = try DocumentTextLoader.load(url: pdf)
        #expect(loaded.contains("Hello PDF"))
    }

    @Test func unsupportedTypeThrows() throws {
        let url = try writeTempFile(named: "note.docx", data: Data("x".utf8))
        #expect(throws: DocumentTextError.unsupportedType("docx")) {
            try DocumentTextLoader.load(url: url)
        }
    }

    @Test func corruptPDFThrows() throws {
        let url = try writeTempFile(named: "broken.pdf", data: Data("not a pdf".utf8))
        #expect(throws: DocumentTextError.self) {
            try DocumentTextLoader.load(url: url)
        }
    }

    /// Minimal one-page PDF with base-14 Helvetica text and a computed xref table.
    private func makePDFData(text: String) -> Data {
        let escaped =
            text
            .replacingOccurrences(of: "\\", with: "\\\\")
            .replacingOccurrences(of: "(", with: "\\(")
            .replacingOccurrences(of: ")", with: "\\)")
        let content = "BT /F1 24 Tf 100 700 Td (\(escaped)) Tj ET"
        var data = Data("%PDF-1.4\n".utf8)
        var offsets: [Int] = []
        func addObject(_ body: String) {
            offsets.append(data.count)
            data.append(contentsOf: "\(offsets.count) 0 obj\n\(body)\nendobj\n".utf8)
        }
        addObject("<< /Type /Catalog /Pages 2 0 R >>")
        addObject("<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
        addObject(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>"
        )
        addObject("<< /Length \(content.utf8.count) >>\nstream\n\(content)\nendstream")
        addObject("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
        let xrefOffset = data.count
        var xref = "xref\n0 \(offsets.count + 1)\n0000000000 65535 f \n"
        for offset in offsets {
            xref += String(format: "%010d 00000 n \n", offset)
        }
        xref += "trailer\n<< /Size \(offsets.count + 1) /Root 1 0 R >>\nstartxref\n\(xrefOffset)\n%%EOF\n"
        data.append(contentsOf: xref.utf8)
        return data
    }
}
