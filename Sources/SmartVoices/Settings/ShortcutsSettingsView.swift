import AppKit
import SmartVoicesCore
import SwiftUI

/// Shortcuts tab: one recorder row per action.
@MainActor
struct ShortcutsSettingsView: View {
    @Bindable var settings: AppSettings
    @Bindable var hotKeyCenter: HotKeyCenter

    var body: some View {
        ScrollView {
            Form {
                Section("Global shortcuts") {
                    ForEach(HotKeyAction.allCases, id: \.self) { action in
                        HStack(alignment: .top) {
                            VStack(alignment: .leading) {
                                Text(title(for: action))
                                Text(caption(for: action))
                                    .font(.callout)
                                    .foregroundStyle(.secondary)
                                if let error = hotKeyCenter.errors[action] {
                                    Text(error)
                                        .font(.callout)
                                        .foregroundStyle(.red)
                                }
                            }
                            Spacer()
                            HotKeyRecorder(binding: Binding(
                                get: { settings.hotKeys[action] },
                                set: { settings.hotKeys[action] = $0 }
                            ))
                        }
                    }
                    Explanation(text: "Click a shortcut, then press the keys you want. Each shortcut must include ⌃, ⌥, or ⌘. Shortcuts work even when Smart Voices is in the background.")
                }
            }
            .formStyle(.grouped)
            .padding()
        }
    }

    private func title(for action: HotKeyAction) -> String {
        switch action {
        case .playPause: return "Play / Pause"
        case .stop: return "Stop"
        case .nextSentence: return "Next sentence"
        case .previousSentence: return "Previous sentence"
        }
    }

    private func caption(for action: HotKeyAction) -> String {
        switch action {
        case .playPause: return "Start reading the clipboard, or pause and resume."
        case .stop: return "Stop reading."
        case .nextSentence: return "Skip to the next sentence while reading."
        case .previousSentence: return "Go back one sentence while reading."
        }
    }
}

/// Button that records the next key press as a shortcut.
@MainActor
struct HotKeyRecorder: View {
    @Binding var binding: HotKey?
    @State private var model = HotKeyRecorderModel()

    var body: some View {
        Button(model.recording ? "Type shortcut…" : (binding?.displayString ?? "Set Shortcut")) {
            model.start { binding = $0 }
        }
        .onDisappear { model.stop() }
    }
}

@MainActor @Observable
private final class HotKeyRecorderModel {
    private(set) var recording = false
    private var monitor: Any?
    private var commit: ((HotKey?) -> Void)?

    isolated deinit {
        if let monitor {
            NSEvent.removeMonitor(monitor)
        }
    }

    func start(commit: @escaping (HotKey?) -> Void) {
        stop()
        self.commit = commit
        recording = true
        let token = Unmanaged.passUnretained(self).toOpaque()
        monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
            MainActor.assumeIsolated {
                Unmanaged<HotKeyRecorderModel>.fromOpaque(token).takeUnretainedValue().handle(event)
            }
            return nil
        }
    }

    func stop() {
        if let monitor {
            NSEvent.removeMonitor(monitor)
        }
        monitor = nil
        recording = false
        commit = nil
    }

    private func handle(_ event: NSEvent) {
        guard recording else { return }
        if event.keyCode == 53 {
            stop()  // Escape cancels.
            return
        }
        if event.keyCode == 51 {
            commit?(nil)  // Delete clears.
            stop()
            return
        }
        let required = event.modifierFlags.intersection([.control, .option, .command])
        guard !required.isEmpty else { return }
        var modifiers: UInt32 = 0
        if required.contains(.control) { modifiers |= HotKey.controlKey }
        if required.contains(.option) { modifiers |= HotKey.optionKey }
        if event.modifierFlags.contains(.shift) { modifiers |= HotKey.shiftKey }
        if required.contains(.command) { modifiers |= HotKey.cmdKey }
        commit?(HotKey(keyCode: UInt32(event.keyCode), modifiers: modifiers))
        stop()
    }
}
