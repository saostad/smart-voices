import Foundation

#if canImport(FoundationModels)
import FoundationModels
#endif

/// Whether conversational rewriting is available on this Mac.
public enum RewriterAvailability: Sendable, Equatable {
    case available
    case unavailable(reason: String)
}

/// Rewrites text into natural spoken style before it is read aloud.
public protocol ConversationRewriter: Sendable {
    func availability() -> RewriterAvailability
    func rewrite(_ text: String) async throws -> String
}

/// Groups sanitized segments into rewrite blocks at segment boundaries.
public enum RewriteBlocks {
    /// Join segment texts with a space into blocks of at most `maxCharacters`.
    /// A single oversized segment stays whole in its own block.
    public static func makeBlocks(
        from segments: [SpeechSegment], maxCharacters: Int = 3000
    ) -> [String] {
        var blocks: [String] = []
        var current = ""
        for segment in segments {
            let text = segment.text.trimmingCharacters(in: .whitespacesAndNewlines)
            if text.isEmpty { continue }
            if current.isEmpty {
                current = text
            } else if current.count + 1 + text.count <= maxCharacters {
                current += " " + text
            } else {
                blocks.append(current)
                current = text
            }
        }
        if !current.isEmpty {
            blocks.append(current)
        }
        return blocks
    }
}

/// The Grok voice-mode style prompt for conversational rewriting.
public enum ConversationalPrompt {
    /// Instructions for the rewriting model.
    public static let instructions = """
        Retell the text below so it sounds natural when read aloud, like a \
        voice assistant speaking conversationally. Rules: use the same language \
        as the input; keep every fact, name, and number unchanged; use short \
        spoken sentences; say nothing about formatting, markup, links, or URLs \
        (mention site names conversationally instead); no emoji; no preamble \
        and no commentary about the rewrite itself. Reply with only the retold \
        text.
        """

    /// The full prompt for one block of text.
    public static func prompt(for text: String) -> String {
        "Retell this for listening:\n\n\(text)"
    }
}

#if canImport(FoundationModels)
/// Rewriter using Apple's on-device model. Nothing leaves this Mac.
@available(macOS 26, *)
public struct FoundationModelsRewriter: ConversationRewriter, Sendable {
    public init() {}

    public func availability() -> RewriterAvailability {
        switch SystemLanguageModel.default.availability {
        case .available:
            return .available
        case .unavailable(let reason):
            return .unavailable(reason: Self.reasonText(for: reason))
        }
    }

    public func rewrite(_ text: String) async throws -> String {
        let session = LanguageModelSession(instructions: ConversationalPrompt.instructions)
        let response = try await session.respond(to: ConversationalPrompt.prompt(for: text))
        return response.content.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private static func reasonText(
        for reason: SystemLanguageModel.Availability.UnavailableReason
    ) -> String {
        switch reason {
        case .deviceNotEligible:
            return "This Mac doesn't support Apple Intelligence."
        case .appleIntelligenceNotEnabled:
            return "Apple Intelligence is turned off. Enable it in System Settings → Apple Intelligence & Siri."
        case .modelNotReady:
            return "The on-device model is still downloading. Try again later."
        @unknown default:
            return "Apple Intelligence is unavailable."
        }
    }
}
#endif

/// Shared rewrite step: playback, export, and headless all fall back the same way.
/// Returns rewritten text, or nil with a notice when reading as-is (nil notice
/// means the call was cancelled and the caller should abort silently).
public enum ConversationalRewrite {
    public static func rewrite(
        _ blocks: [String], with rewriter: any ConversationRewriter,
        isCancelled: @escaping @Sendable () -> Bool = { false }
    ) async -> (text: String?, notice: String?) {
        switch rewriter.availability() {
        case .available:
            break
        case .unavailable(let reason):
            return (
                nil, "Conversational rewrite unavailable: \(reason) Reading as-is."
            )
        }
        var rewritten: [String] = []
        for block in blocks {
            if isCancelled() { return (nil, nil) }
            do {
                rewritten.append(try await rewriter.rewrite(block))
            } catch is CancellationError {
                return (nil, nil)
            } catch {
                return (
                    nil,
                    "Conversational rewrite failed: \(error.localizedDescription). Reading as-is."
                )
            }
            if isCancelled() { return (nil, nil) }
        }
        let joined = rewritten.joined(separator: "\n\n").trimmingCharacters(
            in: .whitespacesAndNewlines)
        if joined.isEmpty {
            return (nil, "Conversational rewrite returned no text. Reading as-is.")
        }
        return (joined, nil)
    }
}

/// A rewriter that is always unavailable. Lets callers attempt a rewrite on any
/// OS and get the same fallback notice instead of branching on availability.
public struct UnavailableRewriter: ConversationRewriter, Sendable {
    public let reason: String
    public init(reason: String) {
        self.reason = reason
    }

    public func availability() -> RewriterAvailability {
        .unavailable(reason: reason)
    }

    public func rewrite(_ text: String) async throws -> String {
        throw RewriteUnavailableError(reason: reason)
    }
}

private struct RewriteUnavailableError: Error, LocalizedError {
    var reason: String
    var errorDescription: String? { reason }
}

/// Entry point the app uses so call sites don't need availability checks.
public enum ConversationRewriteSupport {
    public static func availability() -> RewriterAvailability {
        #if canImport(FoundationModels)
        if #available(macOS 26, *) {
            return FoundationModelsRewriter().availability()
        }
        #endif
        return .unavailable(reason: "Requires macOS 26 with Apple Intelligence.")
    }

    /// A rewriter to attempt playback with: the on-device model when available,
    /// otherwise a placeholder that reports the same unavailability reason.
    public static func makeAttemptedRewriter() -> any ConversationRewriter {
        if let rewriter = makeRewriter() {
            return rewriter
        }
        if case .unavailable(let reason) = availability() {
            return UnavailableRewriter(reason: reason)
        }
        return UnavailableRewriter(reason: "Requires macOS 26 with Apple Intelligence.")
    }

    /// A working rewriter, or nil when rewriting is unavailable.
    public static func makeRewriter() -> (any ConversationRewriter)? {
        #if canImport(FoundationModels)
        if #available(macOS 26, *) {
            let rewriter = FoundationModelsRewriter()
            if rewriter.availability() == .available {
                return rewriter
            }
        }
        #endif
        return nil
    }
}
