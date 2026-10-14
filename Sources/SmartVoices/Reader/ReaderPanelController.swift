import AppKit
import SmartVoicesCore
import SwiftUI

/// Header title for the reading panel, updated without rebuilding the panel.
@MainActor @Observable
final class ReaderHeaderModel {
    var name: String
    var caption: String

    init(name: String = "", caption: String = "") {
        self.name = name
        self.caption = caption
    }
}

/// Owns the floating reading panel. Never steals focus.
@MainActor
final class ReaderPanelController {
    private var panel: NSPanel?
    private let header = ReaderHeaderModel()
    var onDropURLs: (([URL]) -> Void)?
    var onDropText: ((String) -> Void)?

    var isVisible: Bool {
        panel?.isVisible ?? false
    }

    var isMouseOver: Bool {
        guard let panel, panel.isVisible else { return false }
        return panel.frame.contains(NSEvent.mouseLocation)
    }

    func setHeader(name: String, caption: String) {
        header.name = name
        header.caption = caption
    }

    func show(
        playback: PlaybackController,
        header headerText: (name: String, caption: String),
        onJump: @escaping (Int) -> Void,
        onTogglePlayPause: @escaping () -> Void,
        onNext: @escaping () -> Void,
        onPrevious: @escaping () -> Void,
        onSpeed: @escaping (Double) -> Void,
        onClose: @escaping () -> Void
    ) {
        header.name = headerText.name
        header.caption = headerText.caption
        if panel == nil {
            let view = ReaderView(
                playback: playback, header: header,
                onJump: onJump, onTogglePlayPause: onTogglePlayPause,
                onNext: onNext, onPrevious: onPrevious, onSpeed: onSpeed,
                onClose: onClose,
                onDropURLs: { [weak self] urls in self?.onDropURLs?(urls) },
                onDropText: { [weak self] text in self?.onDropText?(text) },
                previewWordIndex: nil)
            let hosting = NSHostingView(rootView: view)
            hosting.translatesAutoresizingMaskIntoConstraints = false
            let effect = NSVisualEffectView()
            effect.material = .popover
            effect.state = .active
            effect.wantsLayer = true
            effect.layer?.cornerRadius = 16
            effect.layer?.masksToBounds = true
            effect.addSubview(hosting)
            NSLayoutConstraint.activate([
                hosting.leadingAnchor.constraint(equalTo: effect.leadingAnchor),
                hosting.trailingAnchor.constraint(equalTo: effect.trailingAnchor),
                hosting.topAnchor.constraint(equalTo: effect.topAnchor),
                hosting.bottomAnchor.constraint(equalTo: effect.bottomAnchor),
            ])
            let panel = Self.makePanel(contentView: effect)
            panel.setFrameAutosaveName("SmartVoicesReader")
            panel.center()
            self.panel = panel
        }
        panel?.orderFront(nil)
    }

    func hide() {
        panel?.orderOut(nil)
    }

    /// Shared by the live panel and its renderer so previews include window insets.
    static func makePanel(contentView: NSView) -> NSPanel {
        let panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 440, height: 300),
            styleMask: [
                .nonactivatingPanel, .titled, .closable, .resizable, .fullSizeContentView,
            ],
            backing: .buffered,
            defer: false)
        panel.isFloatingPanel = true
        panel.level = .floating
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        panel.titlebarAppearsTransparent = true
        panel.titleVisibility = .hidden
        panel.standardWindowButton(.closeButton)?.isHidden = true
        panel.standardWindowButton(.miniaturizeButton)?.isHidden = true
        panel.standardWindowButton(.zoomButton)?.isHidden = true
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.isMovableByWindowBackground = true
        panel.becomesKeyOnlyIfNeeded = true
        panel.hidesOnDeactivate = false
        panel.minSize = NSSize(width: 320, height: 220)
        panel.contentView = contentView
        return panel
    }
}
