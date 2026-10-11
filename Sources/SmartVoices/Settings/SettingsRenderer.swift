import AppKit
import Foundation
import SmartVoicesCore
import SwiftUI

private struct RenderLoginItem: LoginItemControlling {
    var isEnabled: Bool { false }
    func setEnabled(_ enabled: Bool) throws {}
}

private let renderVoices = [
    VoiceInfo(id: "af_heart", name: "Heart — American female", language: "en-US"),
    VoiceInfo(id: "af_sarah", name: "Sarah — American female", language: "en-US"),
    VoiceInfo(id: "am_adam", name: "Adam — American male", language: "en-US"),
    VoiceInfo(id: "bf_emma", name: "Emma — British female", language: "en-GB"),
    VoiceInfo(id: "bm_george", name: "George — British male", language: "en-GB"),
]

/// Render every settings tab in light and dark mode to `<dir>/settings-<tab>-<light|dark>.png`.
@MainActor
func renderSettings(to dir: String) -> Int32 {
    _ = NSApplication.shared
    do {
        try FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
    } catch {
        writeStderr("SmartVoices: cannot create \(dir): \(error.localizedDescription)")
        return 1
    }
    guard let renderDefaults = UserDefaults(suiteName: "sv-render") else {
        writeStderr("SmartVoices: cannot create render defaults")
        return 1
    }
    for key in [
        "sv.engineID", "sv.kokoroVoiceID", "sv.systemVoiceID", "sv.speed", "sv.watchClipboard",
        "sv.codeBlocks", "sv.tables", "sv.bareURLs", "sv.maxTableRowsToRead",
        "sv.minimumReadyLength", "sv.ignoredAppBundleIDs", "sv.hotKeys", "sv.showReaderPanel",
        "sv.keepHistory",
    ] {
        renderDefaults.removeObject(forKey: key)
    }
    let settings = AppSettings(defaults: renderDefaults)
    let voiceModel = VoiceModel(status: .ready, voices: renderVoices)
    let hotKeyCenter = HotKeyCenter()
    // TabView chrome needs a window to lay out, so host each render in a window
    // parked far offscreen. It is ordered front for correct display but never visible.
    let window = NSWindow(
        contentRect: NSRect(x: -10000, y: -10000, width: 600, height: 520),
        styleMask: [.titled, .closable],
        backing: .buffered,
        defer: false)
    window.title = "Smart Voices Settings"
    let appearances: [(NSAppearance.Name, String)] = [(.aqua, "light"), (.darkAqua, "dark")]
    for tab in SettingsTab.allCases {
        for (appearanceName, suffix) in appearances {
            let root = SettingsRootView(
                settings: settings,
                voiceModel: voiceModel,
                hotKeyCenter: hotKeyCenter,
                loginItem: RenderLoginItem(),
                onSelectEngine: { _ in },
                onSelectVoice: { _ in },
                onPreviewVoice: {},
                onClearHistory: {},
                selectedTab: tab)
            let hosting = NSHostingView(rootView: root)
            hosting.frame = NSRect(x: 0, y: 0, width: 600, height: 520)
            let appearance = NSAppearance(named: appearanceName)
            hosting.appearance = appearance
            window.appearance = appearance
            window.contentView = hosting
            window.layoutIfNeeded()
            hosting.layoutSubtreeIfNeeded()
            // SwiftUI puts the TabView tabs in the window toolbar, so capture the
            // whole window frame (title bar + toolbar + 600x520 content).
            guard let frameView = window.contentView?.superview else {
                writeStderr("SmartVoices: cannot render settings tab \(tab.rawValue) (\(suffix))")
                return 1
            }
            guard let rep = frameView.bitmapImageRepForCachingDisplay(in: frameView.bounds) else {
                writeStderr("SmartVoices: cannot render settings tab \(tab.rawValue) (\(suffix))")
                return 1
            }
            frameView.cacheDisplay(in: frameView.bounds, to: rep)
            guard let png = rep.representation(using: .png, properties: [:]) else {
                writeStderr("SmartVoices: cannot encode settings tab \(tab.rawValue) (\(suffix))")
                return 1
            }
            let path = (dir as NSString).appendingPathComponent("settings-\(tab.rawValue)-\(suffix).png")
            do {
                try png.write(to: URL(fileURLWithPath: path))
            } catch {
                writeStderr("SmartVoices: cannot write \(path): \(error.localizedDescription)")
                return 1
            }
            print(path)
        }
    }
    return 0
}
