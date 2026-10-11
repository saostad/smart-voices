import SmartVoicesCore
import SwiftUI

/// Pronunciation tab: teach Smart Voices how to say words.
@MainActor
struct PronunciationSettingsView: View {
    @Bindable var settings: AppSettings
    var onPreviewPronunciation: (String) -> Void
    @State private var selection = Set<PronunciationEntry.ID>()

    var body: some View {
        ScrollView {
            Form {
                Section("Dictionary") {
                    Table($settings.pronunciations, selection: $selection) {
                        TableColumn("Written") { $entry in
                            TextField("Written", text: $entry.written)
                        }
                        TableColumn("Says") { $entry in
                            TextField("Says", text: $entry.spoken)
                        }
                        TableColumn("Whole word") { $entry in
                            Toggle("", isOn: $entry.wholeWord)
                                .labelsHidden()
                        }
                        .width(80)
                        TableColumn("") { $entry in
                            Button {
                                onPreviewPronunciation(entry.spoken)
                            } label: {
                                Image(systemName: "speaker.wave.2")
                            }
                            .buttonStyle(.plain)
                            .disabled(entry.spoken.isEmpty)
                        }
                        .width(36)
                    }
                    .frame(minHeight: 200)
                    HStack {
                        Button {
                            settings.pronunciations.append(
                                PronunciationEntry(written: "", spoken: ""))
                        } label: {
                            Image(systemName: "plus")
                        }
                        Button {
                            settings.pronunciations.removeAll {
                                selection.contains($0.id)
                            }
                            selection.removeAll()
                        } label: {
                            Image(systemName: "minus")
                        }
                        .disabled(selection.isEmpty)
                        Spacer()
                        Button("Restore Defaults") {
                            settings.pronunciations = AppSettings.defaultPronunciations
                            selection.removeAll()
                        }
                    }
                    Explanation(text: "Teach Smart Voices how to say names, acronyms and jargon.")
                }
            }
            .formStyle(.grouped)
            .padding()
        }
    }
}
