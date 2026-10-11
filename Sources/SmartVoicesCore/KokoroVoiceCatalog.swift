import Foundation

/// Display names for Kokoro voice ids, without the model dependency.
public enum KokoroVoiceCatalog: Sendable {
    /// Human-readable name for a Kokoro voice id, or nil if the id is invalid.
    public static func displayName(for id: String) -> String? {
        guard id.count > 3, id.dropFirst(2).first == "_" else { return nil }
        let region: String
        let gender: String
        switch (id.first, id.dropFirst().first) {
        case ("a", "f"): region = "American"; gender = "female"
        case ("a", "m"): region = "American"; gender = "male"
        case ("b", "f"): region = "British"; gender = "female"
        case ("b", "m"): region = "British"; gender = "male"
        default: return nil
        }
        let suffix = id.dropFirst(3)
        guard !suffix.isEmpty else { return nil }
        return "\(suffix.prefix(1).uppercased())\(suffix.dropFirst()) — \(region) \(gender)"
    }
}
