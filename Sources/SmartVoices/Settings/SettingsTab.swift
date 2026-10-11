import SwiftUI

/// Settings window tabs.
enum SettingsTab: String, CaseIterable, Hashable {
    case general
    case voice
    case reading
    case shortcuts
    case about

    var title: String {
        switch self {
        case .general: return "General"
        case .voice: return "Voice"
        case .reading: return "Reading"
        case .shortcuts: return "Shortcuts"
        case .about: return "About"
        }
    }

    var systemImage: String {
        switch self {
        case .general: return "gearshape"
        case .voice: return "waveform"
        case .reading: return "text.bubble"
        case .shortcuts: return "keyboard"
        case .about: return "info.circle"
        }
    }
}

/// Beginner-friendly gray caption shown under every control.
@MainActor
struct Explanation: View {
    var text: String

    var body: some View {
        Text(text)
            .font(.callout)
            .foregroundStyle(.secondary)
    }
}
