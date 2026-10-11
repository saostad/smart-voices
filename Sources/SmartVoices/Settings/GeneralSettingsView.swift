import AppKit
import SmartVoicesCore
import SwiftUI

/// General tab: login item, clipboard watching, ignored apps.
@MainActor
struct GeneralSettingsView: View {
    @Bindable var settings: AppSettings
    var loginItem: any LoginItemControlling
    var onClearHistory: () -> Void
    @State private var launchAtLogin = false
    @State private var loginError: String?

    var body: some View {
        ScrollView {
            Form {
                Section("Startup") {
                    Toggle("Launch at login", isOn: Binding(
                        get: { launchAtLogin },
                        set: { newValue in
                            do {
                                try loginItem.setEnabled(newValue)
                                launchAtLogin = loginItem.isEnabled
                                loginError = nil
                            } catch {
                                loginError = error.localizedDescription
                            }
                        }
                    ))
                    Explanation(text: "Start Smart Voices automatically when you log in.")
                    if let loginError {
                        Text(loginError)
                            .font(.callout)
                            .foregroundStyle(.red)
                    }
                }

                Section("Clipboard") {
                    Toggle("Watch the clipboard", isOn: $settings.watchClipboard)
                    Explanation(text: "Shows a play badge in the menu bar when you copy new text. Smart Voices only reads the clipboard when you press play.")
                    Stepper(
                        "Minimum text length: \(settings.minimumReadyLength) characters",
                        value: $settings.minimumReadyLength, in: 1...1000)
                    Explanation(text: "Shorter copies won't light up the play badge or play.")
                }

                Section("Reading panel") {
                    Toggle("Show reading panel while playing", isOn: $settings.showReaderPanel)
                    Explanation(text: "A small floating window that highlights each sentence and word as it's read.")
                }

                Section("History") {
                    Toggle("Remember recently read text", isOn: $settings.keepHistory)
                    Explanation(text: "Keeps the last 20 items on this Mac so you can replay them.")
                    Button("Clear History") {
                        onClearHistory()
                    }
                    Explanation(text: "Deletes all remembered text right away.")
                }

                Section("Ignored apps") {
                    ForEach(settings.ignoredAppBundleIDs, id: \.self) { bundleID in
                        HStack {
                            Image(nsImage: appIcon(for: bundleID))
                                .resizable()
                                .frame(width: 16, height: 16)
                            VStack(alignment: .leading) {
                                Text(appName(for: bundleID))
                                Text(appCaption(for: bundleID))
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            }
                            Spacer()
                            Button("Remove") {
                                settings.ignoredAppBundleIDs.removeAll { $0 == bundleID }
                            }
                        }
                    }
                    Menu("Add App…") {
                        ForEach(runningApps, id: \.bundleIdentifier) { app in
                            Button(app.localizedName ?? app.bundleIdentifier ?? "Unknown") {
                                if let id = app.bundleIdentifier,
                                    !settings.ignoredAppBundleIDs.contains(id)
                                {
                                    settings.ignoredAppBundleIDs.append(id)
                                }
                            }
                            .disabled(settings.ignoredAppBundleIDs.contains(app.bundleIdentifier ?? ""))
                        }
                    }
                    Explanation(text: "Copies made in these apps (like password managers) never light up the play badge.")
                }
            }
            .formStyle(.grouped)
            .padding()
        }
        .onAppear { launchAtLogin = loginItem.isEnabled }
    }

    private var runningApps: [NSRunningApplication] {
        NSWorkspace.shared.runningApplications
            .filter { $0.activationPolicy == .regular && $0.bundleIdentifier != nil }
            .filter { $0.bundleIdentifier != "dev.smartvoices.app" }
            .sorted { ($0.localizedName ?? "") < ($1.localizedName ?? "") }
    }

    private func appURL(for bundleID: String) -> URL? {
        NSWorkspace.shared.urlForApplication(withBundleIdentifier: bundleID)
    }

    private func appName(for bundleID: String) -> String {
        guard let url = appURL(for: bundleID) else { return bundleID }
        if let bundle = Bundle(url: url),
            let name = (bundle.object(forInfoDictionaryKey: "CFBundleDisplayName")
                ?? bundle.object(forInfoDictionaryKey: "CFBundleName")) as? String,
            !name.isEmpty
        {
            return name
        }
        let display = FileManager.default.displayName(atPath: url.path)
        if display.hasSuffix(".app") {
            return String(display.dropLast(4))
        }
        return display
    }

    private func appCaption(for bundleID: String) -> String {
        appURL(for: bundleID) == nil ? "Not installed" : bundleID
    }

    private func appIcon(for bundleID: String) -> NSImage {
        if let url = appURL(for: bundleID) {
            return NSWorkspace.shared.icon(forFile: url.path)
        }
        return NSImage(named: NSImage.applicationIconName) ?? NSImage()
    }
}
