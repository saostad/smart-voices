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
if let exportIndex = args.firstIndex(of: "--export") {
    if exportIndex + 1 >= args.count {
        writeStderr("SmartVoices: missing argument for --export")
        exit(1)
    }
    let outPath = args[exportIndex + 1]
    if !args.contains("--say") && !args.contains("--say-file") {
        writeStderr("SmartVoices: --export needs --say or --say-file for the text to read")
        exit(1)
    }
    let engine = makeHeadlessEngine(engineID: headlessEngineID(args: args))
    let text = headlessText(args: args)
    Task { @MainActor in
        exit(await runExportAsync(text: text, engine: engine, outputPath: outPath))
    }
    RunLoop.main.run()
    exit(1)
}
if args.contains("--say") || args.contains("--say-file") {
    let engine = makeHeadlessEngine(engineID: headlessEngineID(args: args))
    let text = headlessText(args: args)
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
