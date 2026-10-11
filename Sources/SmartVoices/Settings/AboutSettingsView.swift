import AppKit
import SwiftUI

/// About tab: version, privacy, model folder, credits.
@MainActor
struct AboutSettingsView: View {
    var body: some View {
        ScrollView {
            Form {
                Section("Smart Voices") {
                    Text("Smart Voices \(version)")
                        .font(.headline)
                    Explanation(text: "Runs 100% on this Mac. No text ever leaves your computer.")
                }

                Section("Voice model") {
                    HStack {
                        Text(modelURL.path)
                            .font(.callout)
                            .lineLimit(1)
                            .truncationMode(.middle)
                        Spacer()
                        Button("Open in Finder") {
                            NSWorkspace.shared.open(modelURL)
                        }
                    }
                    Explanation(text: "Voice models download here on first launch (about 120 MB).")
                }

                Section("Credits") {
                    Text("Voice: Kokoro-82M (Apache 2.0)")
                    if let fluidURL = URL(string: "https://github.com/FluidInference/FluidAudio") {
                        Link("Engine: FluidAudio on GitHub", destination: fluidURL)
                    }
                    Explanation(text: "Thanks to the open-source projects that make on-device speech possible.")
                }
            }
            .formStyle(.grouped)
            .padding()
        }
    }

    private var version: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "0.1.0"
    }

    private var modelURL: URL {
        FileManager.default.homeDirectoryForCurrentUser.appending(path: ".cache/fluidaudio")
    }
}
