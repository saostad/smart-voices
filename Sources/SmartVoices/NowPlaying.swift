import Foundation
import MediaPlayer
import SmartVoicesCore

/// Media keys (play/pause/next/previous/stop) and Now Playing info for
/// Control Center. All handlers run on the main actor.
@MainActor
final class NowPlayingController {
    var onPlay: (() -> Void)?
    var onPause: (() -> Void)?
    var onToggle: (() -> Void)?
    var onNext: (() -> Void)?
    var onPrevious: (() -> Void)?
    var onStop: (() -> Void)?

    private var registrations: [(command: MPRemoteCommand, token: Any)] = []

    init() {
        let center = MPRemoteCommandCenter.shared()
        install(center.playCommand) { $0.onPlay?() }
        install(center.pauseCommand) { $0.onPause?() }
        install(center.togglePlayPauseCommand) { $0.onToggle?() }
        install(center.nextTrackCommand) { $0.onNext?() }
        install(center.previousTrackCommand) { $0.onPrevious?() }
        install(center.stopCommand) { $0.onStop?() }
    }

    isolated deinit {
        for (command, token) in registrations {
            command.removeTarget(token)
        }
    }

    /// Refresh Now Playing info from playback state. Clears it when idle.
    func update(
        state: PlaybackController.PlaybackState, chunks: [SpeechChunk], index: Int,
        speed: Double
    ) {
        guard state != .idle, !chunks.isEmpty else {
            MPNowPlayingInfoCenter.default().nowPlayingInfo = nil
            return
        }
        MPNowPlayingInfoCenter.default().nowPlayingInfo = [
            MPMediaItemPropertyTitle: String(chunks[0].text.prefix(60)),
            MPMediaItemPropertyArtist: "Smart Voices",
            MPNowPlayingInfoPropertyPlaybackRate: state == .playing ? speed : 0,
            MPNowPlayingInfoPropertyChapterNumber: index,
            MPNowPlayingInfoPropertyChapterCount: chunks.count,
        ]
    }

    private func install(
        _ command: MPRemoteCommand, fire: @escaping @MainActor (NowPlayingController) -> Void
    ) {
        command.isEnabled = true
        let token = command.addTarget { [weak self] _ in
            guard let self else { return .noActionableNowPlayingItem }
            Task { @MainActor in fire(self) }
            return .success
        }
        registrations.append((command, token))
    }
}
