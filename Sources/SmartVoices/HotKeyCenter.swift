import AppKit
import Carbon
import os
import SmartVoicesCore

private let hotKeySignature: OSType = 0x5356_6F31  // "SVo1"

private extension HotKeyAction {
    var hotKeyID: UInt32 {
        UInt32(HotKeyAction.allCases.firstIndex(of: self) ?? 0)
    }
}

/// Global keyboard shortcuts via Carbon hotkeys (no Accessibility permission needed).
@MainActor @Observable
final class HotKeyCenter {
    var onAction: (@MainActor (HotKeyAction) -> Void)?
    private(set) var errors: [HotKeyAction: String] = [:]

    private var refs: [HotKeyAction: EventHotKeyRef?] = [:]
    private var eventHandler: EventHandlerRef?
    private let logger = Logger(subsystem: "dev.smartvoices", category: "hotkeys")

    isolated deinit {
        for ref in refs.values {
            if let ref {
                UnregisterEventHotKey(ref)
            }
        }
        if let eventHandler {
            RemoveEventHandler(eventHandler)
        }
    }

    /// Re-register every binding. A missing entry means disabled.
    func updateBindings(_ bindings: [HotKeyAction: HotKey]) {
        unregisterAll()
        errors = [:]
        installHandlerIfNeeded()
        for action in HotKeyAction.allCases {
            guard let binding = bindings[action] else { continue }
            var ref: EventHotKeyRef?
            let hotKeyID = EventHotKeyID(signature: hotKeySignature, id: action.hotKeyID)
            let status = RegisterEventHotKey(
                binding.keyCode, binding.modifiers, hotKeyID, GetApplicationEventTarget(), 0, &ref)
            if status == noErr, let ref {
                refs[action] = ref
            } else {
                errors[action] =
                    "\(binding.displayString) is already used by another app. Pick a different shortcut in Settings → Shortcuts."
            }
        }
    }

    func handleHotKeyID(_ id: UInt32) {
        guard let action = HotKeyAction.allCases.first(where: { $0.hotKeyID == id }) else { return }
        onAction?(action)
    }

    private func unregisterAll() {
        for ref in refs.values {
            if let ref {
                UnregisterEventHotKey(ref)
            }
        }
        refs = [:]
    }

    private func installHandlerIfNeeded() {
        guard eventHandler == nil else { return }
        var spec = EventTypeSpec(
            eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
        let userData = Unmanaged.passUnretained(self).toOpaque()
        let handler: EventHandlerUPP = { _, event, userData in
            guard let event, let userData else { return OSStatus(eventNotHandledErr) }
            var hotKeyID = EventHotKeyID()
            let status = GetEventParameter(
                event, EventParamName(kEventParamDirectObject), EventParamType(typeEventHotKeyID),
                nil, MemoryLayout<EventHotKeyID>.size, nil, &hotKeyID)
            guard status == noErr else { return status }
            let center = Unmanaged<HotKeyCenter>.fromOpaque(userData).takeUnretainedValue()
            Task { @MainActor in center.handleHotKeyID(hotKeyID.id) }
            return noErr
        }
        var installed: EventHandlerRef?
        let status = InstallEventHandler(
            GetApplicationEventTarget(), handler, 1, &spec, userData, &installed)
        if status == noErr {
            eventHandler = installed
        } else {
            logger.error("Couldn't install the global shortcut handler (OSStatus \(status, privacy: .public)).")
        }
    }
}
