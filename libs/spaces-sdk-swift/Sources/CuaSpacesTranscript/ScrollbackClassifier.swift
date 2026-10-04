import CuaSpaces
import Foundation

// `CuaSpacesTranscript` is the guessing module, and importing it is the act of
// consent.
//
// The Spaces harness publishes one thing about a run's output: `output_tail`,
// a window of terminal scrollback. Turning that into tool cards, questions and
// file chips is **inference**, not decoding. `CuaSpaces` will not do it, so
// that nothing a product renders as an agent's words is something the SDK made
// up.
//
// This module will. Every event it produces carries `isInferred == true`, and a
// UI is free to render an inferred `.question` as a plain line of text. One
// shared guess, opted into and labelled, instead of a private parser in every
// app that each look like facts.

/// A classifier that reads terminal scrollback and guesses at structure.
///
/// **Everything it returns is a guess.** It is deliberately conservative: a
/// line it is not confident about stays plain `.text`, because a wrong
/// `.question` puts a prompt in front of a user that the agent never asked.
public struct ScrollbackClassifier: AgentTranscriptClassifying {

    /// How eager to be. The default refuses more than it accepts.
    public enum Confidence: Sendable {
        /// Only patterns the two shipping harnesses actually emit.
        case conservative
        /// Also common conventions. More cards, more wrong cards.
        case eager
    }

    public let confidence: Confidence

    public init(confidence: Confidence = .conservative) {
        self.confidence = confidence
    }

    public func classify(line: String, in run: RunID) -> AgentEvent.Kind? {
        let trimmed = line.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty else { return nil }

        // Claude Code and Codex both bracket a tool invocation. This is the one
        // pattern confident enough for the conservative tier.
        if let tool = Self.toolInvocation(trimmed) {
            return .toolUse(name: tool.name, summary: tool.summary)
        }

        guard confidence == .eager else { return nil }

        // A question, guessed. Never conservative: a wrong guess here pops a
        // prompt the agent did not ask, which is the failure this whole module
        // is labelled against.
        if trimmed.hasSuffix("?"), trimmed.count < 240,
           Self.questionOpeners.contains(where: {
               trimmed.lowercased().hasPrefix($0)
           }) {
            return .question(trimmed)
        }

        // A path the agent said it wrote.
        if let path = Self.writtenPath(trimmed) {
            return .artifact(RemoteFile(path: path,
                                        name: (path as NSString).lastPathComponent,
                                        byteCount: nil))
        }
        return nil
    }

    private static let questionOpeners = [
        "should i", "would you like", "do you want", "shall i", "which ",
        "is it ok", "can you confirm",
    ]

    /// `⏺ Bash(ls -la)` / `● Read(src/main.swift)` — the bracketed form both
    /// shipping harnesses print.
    static func toolInvocation(_ line: String) -> (name: String, summary: String)? {
        var body = Substring(line)
        for bullet in ["⏺ ", "● ", "• "] where body.hasPrefix(bullet) {
            body = body.dropFirst(bullet.count)
        }
        guard body.count != line.count || body.first?.isUppercase == true else { return nil }
        guard let open = body.firstIndex(of: "("), body.hasSuffix(")") else { return nil }
        let name = String(body[..<open]).trimmingCharacters(in: .whitespaces)
        guard !name.isEmpty, name.count <= 24,
              name.allSatisfy({ $0.isLetter || $0.isNumber || $0 == "_" })
        else { return nil }
        let inner = body[body.index(after: open)..<body.index(before: body.endIndex)]
        return (name, String(inner))
    }

    static func writtenPath(_ line: String) -> String? {
        let lowered = line.lowercased()
        guard lowered.hasPrefix("wrote ") || lowered.hasPrefix("created ")
                || lowered.hasPrefix("saved ") else { return nil }
        guard let token = line.split(separator: " ").dropFirst().first else { return nil }
        let path = token.trimmingCharacters(in: CharacterSet(charactersIn: "'\"`.,"))
        guard path.contains("/") || path.contains(".") else { return nil }
        return path
    }
}

extension AgentRun {
    /// This run's output, with structure guessed out of scrollback.
    ///
    /// Identical to `events(since:)` except that a classifier is installed, so
    /// richer kinds appear — and every one of them says `isInferred == true`.
    public func inferredEvents(since cursor: OutputCursor = .beginning,
                               pollingEvery interval: Duration = .seconds(1),
                               confidence: ScrollbackClassifier.Confidence = .conservative)
        -> AsyncStream<AgentEvent> {
        events(since: cursor, pollingEvery: interval,
               classifier: ScrollbackClassifier(confidence: confidence))
    }
}
