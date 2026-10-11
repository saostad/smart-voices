import Foundation
import SmartVoicesCore

/// Observable voice state for the Settings window.
@MainActor @Observable
final class VoiceModel {
    enum Status: Equatable {
        case ready
        case loading
        case failed(String)
    }

    var status: Status = .loading
    var voices: [VoiceInfo] = []

    init(status: Status = .loading, voices: [VoiceInfo] = []) {
        self.status = status
        self.voices = voices
    }
}
