import AppKit
import Foundation

let args = CommandLine.arguments
if let renderIndex = args.firstIndex(of: "--render-settings") {
    if renderIndex + 1 >= args.count {
        writeStderr("SmartVoices: missing argument for --render-settings")
        exit(1)
    }
    let dir = args[renderIndex + 1]
    let code: Int32 = MainActor.assumeIsolated { renderSettings(to: dir) }
    exit(code)
}
if let readerIndex = args.firstIndex(of: "--render-reader") {
    if readerIndex + 1 >= args.count {
        writeStderr("SmartVoices: missing argument for --render-reader")
        exit(1)
    }
    let png = args[readerIndex + 1]
    let code: Int32 = MainActor.assumeIsolated { renderReader(to: png) }
    exit(code)
}
let sayIndex = args.firstIndex(of: "--say")
let sayFileIndex = args.firstIndex(of: "--say-file")
if sayIndex != nil || sayFileIndex != nil {
    var engineID = "kokoro"
    if let engineIndex = args.firstIndex(of: "--engine") {
        if engineIndex + 1 >= args.count {
            writeStderr("SmartVoices: missing argument for --engine")
            exit(1)
        }
        engineID = args[engineIndex + 1]
    }
    if engineID != "kokoro" && engineID != "system" {
        writeStderr("SmartVoices: unknown engine '\(engineID)'. Use kokoro or system.")
        exit(1)
    }
    let text: String
    if let index = sayIndex {
        if index + 1 >= args.count {
            writeStderr("SmartVoices: missing argument for --say")
            exit(1)
        }
        text = args[index + 1]
    } else if let index = sayFileIndex {
        if index + 1 >= args.count {
            writeStderr("SmartVoices: missing argument for --say-file")
            exit(1)
        }
        let path = args[index + 1]
        do {
            text = try String(contentsOfFile: path, encoding: .utf8)
        } catch {
            writeStderr("SmartVoices: cannot read \(path): \(error.localizedDescription)")
            exit(1)
        }
    } else {
        writeStderr("SmartVoices: missing argument for --say")
        exit(1)
    }
    let engine = makeHeadlessEngine(engineID: engineID)
    Task { @MainActor in
        exit(await runHeadlessAsync(text: text, engine: engine))
    }
    RunLoop.main.run()
    exit(1)
}

let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let delegate = AppDelegate()
app.delegate = delegate
app.run()
