import SmartVoicesCore
import SwiftUI

/// Reading tab: how Markdown is spoken, with a live preview.
@MainActor
struct ReadingSettingsView: View {
    @Bindable var settings: AppSettings
    @State private var sample = Self.defaultSample

    static let defaultSample = """
        # Morning briefing

        Here's your **update** for today.

        - Coffee is ready
        - Meeting at noon

        | City | Temp | Sky |
        | --- | --- | --- |
        | Oslo | 12 | Clear |
        | Paris | 18 | Cloudy |
        | Tokyo | 22 | Rain |

        ```python
        print("hello")
        ```

        Read more at [Example News](https://www.example.com/news).
        """

    var body: some View {
        ScrollView {
            Form {
                Section("Reading aloud") {
                    Picker("Code blocks", selection: $settings.codeBlocks) {
                        Text("Announce").tag("announce")
                        Text("Skip").tag("skip")
                        Text("Read aloud").tag("readFull")
                    }
                    Explanation(text: "Code is hard to listen to. Announce keeps your place without reading it.")
                    Picker("Tables", selection: $settings.tables) {
                        Text("Read rows").tag("readRows")
                        Text("Announce only").tag("announce")
                        Text("Skip").tag("skip")
                    }
                    Explanation(text: "How tables are read aloud: row by row, or just announced.")
                    Picker("Links", selection: $settings.bareURLs) {
                        Text("Say site name").tag("domainOnly")
                        Text("Skip").tag("skip")
                    }
                    Explanation(text: "Web addresses are shortened to the site name, like “link to example.com”.")
                    Stepper(
                        "Max table rows to read: \(settings.maxTableRowsToRead)",
                        value: $settings.maxTableRowsToRead, in: 1...100)
                    Explanation(text: "Longer tables are summarized instead of read row by row.")
                }

                Section("Conversational") {
                    Toggle(
                        "Rewrite text conversationally before reading",
                        isOn: $settings.conversationalMode)
                    Explanation(text: "Retells text in a natural spoken style, like Grok voice mode. Uses Apple's on-device model; nothing leaves this Mac.")
                    if settings.conversationalMode,
                        case .unavailable(let reason) = ConversationRewriteSupport.availability()
                    {
                        Text("Not available: \(reason)")
                            .font(.callout)
                            .foregroundStyle(.orange)
                    }
                }

                Section("Preview") {
                    TextEditor(text: $sample)
                        .frame(minHeight: 120)
                        .font(.body)
                        .border(.separator)
                    Text("Spoken result")
                        .font(.headline)
                    Text(spokenLines)
                        .font(.callout)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(8)
                        .background(Color(nsColor: .textBackgroundColor))
                        .clipShape(RoundedRectangle(cornerRadius: 6))
                    Explanation(text: "This is exactly what Smart Voices will say.")
                }
            }
            .formStyle(.grouped)
            .padding()
        }
    }

    private var spokenLines: String {
        let sanitizer = SpeechSanitizer(options: settings.sanitizerOptions)
        let chunks = SpeechChunker().chunks(from: sanitizer.sanitize(sample))
        if chunks.isEmpty {
            return "(Nothing to read.)"
        }
        return chunks.map { "[\(String(describing: $0.pauseAfter))] \($0.text)" }.joined(separator: "\n")
    }
}
