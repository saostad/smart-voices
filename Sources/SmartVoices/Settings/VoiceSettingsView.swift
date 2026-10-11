import SmartVoicesCore
import SwiftUI

/// Voice tab: engine, voice, preview, speed.
@MainActor
struct VoiceSettingsView: View {
    @Bindable var settings: AppSettings
    @Bindable var voiceModel: VoiceModel
    var onSelectEngine: (String) -> Void
    var onSelectVoice: (String?) -> Void
    var onPreviewVoice: () -> Void

    var body: some View {
        ScrollView {
            Form {
                Section("Engine") {
                    Picker("Engine", selection: Binding(
                        get: { settings.engineID },
                        set: { onSelectEngine($0) }
                    )) {
                        Text("Kokoro (natural)").tag("kokoro")
                        Text("macOS Voice").tag("system")
                    }
                    Explanation(text: "Kokoro is the natural on-device voice. macOS Voice uses the built-in system voices.")
                    HStack {
                        Text("Status:")
                        switch voiceModel.status {
                        case .ready:
                            Text("Ready")
                        case .loading:
                            ProgressView()
                                .controlSize(.small)
                            Text("Loading the voice model…")
                        case .failed(let message):
                            Text(message)
                                .foregroundStyle(.red)
                        }
                    }
                    Explanation(text: "The voice model loads once when Smart Voices starts.")
                }

                Section("Voice") {
                    Picker("Voice", selection: Binding<String?>(
                        get: { currentVoiceID },
                        set: { onSelectVoice($0) }
                    )) {
                        if settings.engineID == "system" {
                            Text("Default").tag(nil as String?)
                        }
                        ForEach(voiceModel.voices) { voice in
                            Text(voice.name).tag(voice.id as String?)
                        }
                    }
                    Explanation(text: "The voice used when reading your clipboard aloud.")
                    Button("Preview") {
                        onPreviewVoice()
                    }
                    Explanation(text: "Hear the selected voice read a short sample.")
                }

                Section("Speed") {
                    HStack {
                        Text("Speed")
                        Slider(
                            value: Binding(
                                get: { settings.speed },
                                set: { settings.speed = ($0 * 20).rounded() / 20 }
                            ), in: 0.5...3.0)
                        Text(String(format: "%g×", settings.speed))
                            .frame(minWidth: 48, alignment: .trailing)
                        if settings.speed != 1.0 {
                            Button("Reset") {
                                settings.speed = 1.0
                            }
                            .controlSize(.small)
                        }
                    }
                    Explanation(text: "How fast Smart Voices reads. Applies right away.")
                }
            }
            .formStyle(.grouped)
            .padding()
        }
    }

    private var currentVoiceID: String? {
        if settings.engineID == "kokoro" {
            return settings.kokoroVoiceID
        }
        return settings.systemVoiceID
    }
}
