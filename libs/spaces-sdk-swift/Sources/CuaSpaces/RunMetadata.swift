import Foundation

/// The one place a run's application metadata is smuggled through the prompt.
///
/// `FRICTION.md` §21 and §41 both ask for `metadata: [String: String]` on
/// `agent_start`, echoed by `agent_list` and `agent_status`. The server does
/// not have it. Until it does, the prompt is the only carrier — the prompt is
/// echoed back as `summary` — and OpenKoalaBots ended up smuggling **two**
/// separate markers (`[openkoalabots:<botID>]` and `[routine]`) through it, both
/// visible to the agent and both needing stripping at every display site, which
/// the live suite caught being missed in exactly one place.
///
/// The SDK does not make that problem go away; it makes it *one* problem.
/// Encoding and stripping happen here and nowhere else, the marker is a single
/// trailing comment rather than a prefix in the sentence the agent reads, and
/// every `summary` the SDK publishes has already been through `strip`.
public enum RunMetadata {
    /// Recognisable, greppable, and last — so an agent reading the prompt meets
    /// the instruction first and the bookkeeping after it.
    static let openMarker = "<!--cua-spaces-meta:"
    static let closeMarker = "-->"

    /// The prompt as it should be sent.
    public static func encode(prompt: String, metadata: [String: String]) -> String {
        guard !metadata.isEmpty else { return prompt }
        let pairs = metadata.keys.sorted().map { "\(escape($0))=\(escape(metadata[$0]!))" }
        return prompt + "\n\n" + openMarker + pairs.joined(separator: ";") + closeMarker
    }

    /// The metadata carried by a prompt echo (a `summary`), if any.
    public static func decode(from text: String) -> [String: String] {
        guard let open = text.range(of: openMarker),
              let close = text.range(of: closeMarker, range: open.upperBound..<text.endIndex)
        else { return [:] }
        let body = String(text[open.upperBound..<close.lowerBound])
        var out: [String: String] = [:]
        for pair in body.split(separator: ";") {
            guard let eq = pair.firstIndex(of: "=") else { continue }
            out[unescape(String(pair[..<eq]))] = unescape(String(pair[pair.index(after: eq)...]))
        }
        return out
    }

    /// The text with the marker removed, for anything a person will read.
    public static func strip(_ text: String) -> String {
        guard let open = text.range(of: openMarker),
              let close = text.range(of: closeMarker, range: open.upperBound..<text.endIndex)
        else { return text }
        var copy = text
        copy.removeSubrange(open.lowerBound..<close.upperBound)
        return copy.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private static func escape(_ s: String) -> String {
        s.replacingOccurrences(of: "%", with: "%25")
            .replacingOccurrences(of: ";", with: "%3B")
            .replacingOccurrences(of: "=", with: "%3D")
            .replacingOccurrences(of: "-->", with: "%2D%2D%3E")
    }

    private static func unescape(_ s: String) -> String {
        s.replacingOccurrences(of: "%2D%2D%3E", with: "-->")
            .replacingOccurrences(of: "%3D", with: "=")
            .replacingOccurrences(of: "%3B", with: ";")
            .replacingOccurrences(of: "%25", with: "%")
    }
}
