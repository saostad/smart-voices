import AppKit

/// macOS Service provider: "Read Aloud with Smart Voices" for selected text.
@MainActor
@objc final class ServicesProvider: NSObject {
    var onReadSelection: ((String) -> Void)?
    var onServiceError: ((String) -> Void)?

    /// Service handler for NSMessage `readSelection`. Called on the main thread.
    @objc func readSelection(
        _ pboard: NSPasteboard, userData: String,
        error: AutoreleasingUnsafeMutablePointer<NSString?>
    ) {
        guard let text = pboard.string(forType: .string), !text.isEmpty else {
            let reason =
                pboard.string(forType: .string) == nil
                ? "no text was provided." : "the selection was empty."
            let message = "The selected text couldn't be read: \(reason)"
            error.pointee = message as NSString
            onServiceError?(message)
            return
        }
        onReadSelection?(text)
    }
}
