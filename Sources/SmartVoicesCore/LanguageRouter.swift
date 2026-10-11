import Foundation
import NaturalLanguage

/// Detects the dominant language of text so playback can pick a matching voice.
public enum LanguageRouter {
    /// Dominant language of the first 1,000 sanitized chars, or nil when the
    /// text has fewer than 12 letters or confidence is below 0.6.
    public static func dominantLanguage(of text: String) -> (code: String, confidence: Double)? {
        let sanitized = SpeechSanitizer().sanitize(text).map(\.text).joined(separator: " ")
        let sample = String(sanitized.prefix(1000))
        let letters = sample.unicodeScalars.filter { CharacterSet.letters.contains($0) }.count
        guard letters >= 12 else { return nil }
        let recognizer = NLLanguageRecognizer()
        recognizer.processString(sample)
        guard let (language, confidence) = recognizer.languageHypotheses(withMaximum: 1).first,
            confidence >= 0.6
        else {
            return nil
        }
        var code = language.rawValue
        // The recognizer labels Persian as Arabic; Persian-only letters disambiguate.
        if code == "ar",
            sample.unicodeScalars.contains(where: { Self.persianSpecificLetters.contains($0) })
        {
            code = "fa"
        }
        return (code, confidence)
    }

    /// Letters used in Persian but not in Arabic (پ چ ژ گ ک ی).
    private static let persianSpecificLetters: Set<Unicode.Scalar> = [
        "\u{067E}", "\u{0686}", "\u{0698}", "\u{06AF}", "\u{06A9}", "\u{06CC}",
    ]
}

/// Shared language-routing messages.
public enum LanguageRouting {
    /// "No <Language> voice is installed. …" for a BCP-47 language code.
    public static func missingVoiceMessage(for code: String) -> String {
        let name = Locale.current.localizedString(forLanguageCode: code) ?? code
        return
            "No \(name) voice is installed. Add one in System Settings → Accessibility → Spoken Content → System Voice → Manage Voices."
    }
}
