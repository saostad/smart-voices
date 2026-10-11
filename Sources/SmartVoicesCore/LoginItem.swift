import Foundation
import ServiceManagement

/// Controls launch at login.
public protocol LoginItemControlling: Sendable {
    var isEnabled: Bool { get }
    func setEnabled(_ enabled: Bool) throws
}

/// Login-item control via SMAppService.
public final class SMAppServiceLoginItem: LoginItemControlling, Sendable {
    public init() {}

    public var isEnabled: Bool {
        SMAppService.mainApp.status == .enabled
    }

    public func setEnabled(_ enabled: Bool) throws {
        do {
            if enabled {
                try SMAppService.mainApp.register()
            } else {
                try SMAppService.mainApp.unregister()
            }
        } catch {
            throw LoginItemError.changeFailed(
                "Couldn't change Launch at Login: \(error.localizedDescription). Move Smart Voices to the Applications folder and try again."
            )
        }
    }
}

/// Login-item errors with user-facing messages.
public enum LoginItemError: Error, Sendable, Equatable {
    case changeFailed(String)
}

extension LoginItemError: LocalizedError {
    public var errorDescription: String? {
        switch self {
        case .changeFailed(let message):
            return message
        }
    }
}
