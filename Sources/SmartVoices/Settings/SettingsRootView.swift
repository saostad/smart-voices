import SmartVoicesCore
import SwiftUI

/// Settings window content: one tab per area.
@MainActor
struct SettingsRootView: View {
    @Bindable var settings: AppSettings
    @Bindable var voiceModel: VoiceModel
    @Bindable var hotKeyCenter: HotKeyCenter
    var loginItem: any LoginItemControlling
    var onSelectEngine: (String) -> Void
    var onSelectVoice: (String?) -> Void
    var onPreviewVoice: () -> Void
    var onPreviewPronunciation: (String) -> Void
    var onClearHistory: () -> Void
    @State var selectedTab: SettingsTab = .general

    var body: some View {
        TabView(selection: $selectedTab) {
            Tab("General", systemImage: "gearshape", value: SettingsTab.general) {
                tabContent(.general)
            }
            Tab("Voice", systemImage: "waveform", value: SettingsTab.voice) {
                tabContent(.voice)
            }
            Tab("Reading", systemImage: "text.bubble", value: SettingsTab.reading) {
                tabContent(.reading)
            }
            Tab("Pronunciation", systemImage: "character.bubble", value: SettingsTab.pronunciation) {
                tabContent(.pronunciation)
            }
            Tab("Shortcuts", systemImage: "keyboard", value: SettingsTab.shortcuts) {
                tabContent(.shortcuts)
            }
            Tab("About", systemImage: "info.circle", value: SettingsTab.about) {
                tabContent(.about)
            }
        }
        .frame(width: 600, height: 520)
    }

    @ViewBuilder
    private func tabContent(_ tab: SettingsTab) -> some View {
        switch tab {
        case .general:
            GeneralSettingsView(
                settings: settings, loginItem: loginItem, onClearHistory: onClearHistory)
        case .voice:
            VoiceSettingsView(
                settings: settings, voiceModel: voiceModel,
                onSelectEngine: onSelectEngine, onSelectVoice: onSelectVoice,
                onPreviewVoice: onPreviewVoice)
        case .reading:
            ReadingSettingsView(settings: settings)
        case .pronunciation:
            PronunciationSettingsView(
                settings: settings, onPreviewPronunciation: onPreviewPronunciation)
        case .shortcuts:
            ShortcutsSettingsView(settings: settings, hotKeyCenter: hotKeyCenter)
        case .about:
            AboutSettingsView()
        }
    }
}
