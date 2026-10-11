import Foundation
import SmartVoicesCore

func writeStderr(_ message: String) {
    if let data = (message + "\n").data(using: .utf8) {
        FileHandle.standardError.write(data)
    }
}

let input: String
if CommandLine.arguments.count > 1 {
    let path = CommandLine.arguments[1]
    do {
        input = try String(contentsOfFile: path, encoding: .utf8)
    } catch {
        writeStderr("svtext: cannot read \(path): \(error.localizedDescription)")
        exit(1)
    }
} else {
    let data = FileHandle.standardInput.readDataToEndOfFile()
    input = String(data: data, encoding: .utf8) ?? ""
}

let sanitizer = SpeechSanitizer()
let chunker = SpeechChunker()
for chunk in chunker.chunks(from: sanitizer.sanitize(input)) {
    print("[\(chunk.pauseAfter)] \(chunk.text)")
}
