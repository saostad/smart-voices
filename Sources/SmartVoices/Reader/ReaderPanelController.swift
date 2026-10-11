import AppKit
import SmartVoicesCore
import SwiftUI

/// Header title for the reading panel, updated without rebuilding the panel.
@MainActor @Observable
final class ReaderHeaderModel {
    var title: String

    init(title: String = "") {
        self.title = title
    }
}

/// Owns the floating reading panel. Never steals focus.
@MainActor
final class ReaderPanelController {
    private var panel: NSPanel?
    private let header = ReaderHeaderModel()

    var isVisible: Bool {
        panel?.isVisible ?? false
    }

    var isMouseOver: Bool {
        guard let panel, panel.isVisible else { return false }
        return panel.frame.contains(NSEvent.mouseLocation)
    }

    func setTitle(_ title: String) {
        header.title = title
    }

    func show(
        playback: PlaybackController,
        title: String,
        onJump: @escaping (Int) -> Void,
        onTogglePlayPause: @escaping () -> Void,
        onNext: @escaping () -> Void,
        onPrevious: @escaping () -> Void,
        onSpeed: @escaping (Double) -> Void,
        onClose: @escaping () -> Void
    ) {
        header.title = title
        if panel == nil {
            let view = ReaderView(
                playback: playback, header: header,
                onJump: onJump, onTogglePlayPause: onTogglePlayPause,
                onNext: onNext, onPrevious: onPrevious, onSpeed: onSpeed,
                onClose: onClose, previewWordIndex: nil)
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
            panel.setFrameAutosaveName("SmartVoicesReader")
            panel.contentView = effect
            panel.center()
            self.panel = panel
        }
        panel?.orderFront(nil)
    }

    func hide() {
        panel?.orderOut(nil)
    }
}
