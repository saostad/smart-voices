import AppKit
import SmartVoicesCore
import SwiftUI

/// Owns the single Settings window.
@MainActor
final class SettingsWindowController {
    private var window: NSWindow?

    func show(
        settings: AppSettings,
        voiceModel: VoiceModel,
        hotKeyCenter: HotKeyCenter,
        loginItem: any LoginItemControlling,
        onSelectEngine: @escaping (String) -> Void,
        onSelectVoice: @escaping (String?) -> Void,
        onPreviewVoice: @escaping () -> Void
    ) {
        if window == nil {
            let root = SettingsRootView(
                settings: settings,
                voiceModel: voiceModel,
                hotKeyCenter: hotKeyCenter,
                loginItem: loginItem,
                onSelectEngine: onSelectEngine,
                onSelectVoice: onSelectVoice,
                onPreviewVoice: onPreviewVoice)
            let hosting = NSHostingView(rootView: root)
            hosting.frame = NSRect(x: 0, y: 0, width: 600, height: 520)
            let window = NSWindow(
                contentRect: NSRect(x: 0, y: 0, width: 600, height: 520),
                styleMask: [.titled, .closable, .miniaturizable],
                backing: .buffered,
                defer: false)
            window.title = "Smart Voices Settings"
            window.contentView = hosting
            window.center()
            window.isReleasedWhenClosed = false
            self.window = window
        }
        window?.makeKeyAndOrderFront(nil)
        NSApp.activate()
    }
}
