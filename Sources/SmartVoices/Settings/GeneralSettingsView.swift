import AppKit
import SmartVoicesCore
import SwiftUI

/// General tab: login item, clipboard watching, ignored apps.
@MainActor
struct GeneralSettingsView: View {
    @Bindable var settings: AppSettings
    var loginItem: any LoginItemControlling
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

                Section("Ignored apps") {
                    ForEach(settings.ignoredAppBundleIDs, id: \.self) { bundleID in
                        HStack {
                            VStack(alignment: .leading) {
                                Text(appName(for: bundleID))
                                if appName(for: bundleID) != bundleID {
                                    Text(bundleID)
                                        .font(.caption)
                                        .foregroundStyle(.secondary)
                                }
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

    private func appName(for bundleID: String) -> String {
        NSWorkspace.shared.runningApplications.first(where: { $0.bundleIdentifier == bundleID })?
            .localizedName ?? bundleID
    }
}
