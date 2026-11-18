import Foundation

/// A monotonic position in one run's output.
///
/// Opaque on purpose: today it is a line offset into the transcript the SDK has
/// assembled from successive tails, and it must be free to become a
/// server-issued token without a source break.
public struct OutputCursor: Sendable, Hashable, Comparable, Codable,
                            CustomStringConvertible {
    let line: Int
    init(line: Int) { self.line = line }

    /// Everything this SDK has seen, from the start.
    public static let beginning = OutputCursor(line: 0)
    /// "From now on." Nothing already in the window is replayed.
    public static let live = OutputCursor(line: Int.max)

    public static func < (a: OutputCursor, b: OutputCursor) -> Bool { a.line < b.line }
    public var description: String { self == .live ? "live" : "line \(line)" }
}

/// One page of a run's output.
public struct OutputPage: Sendable, Hashable {
    public let events: [AgentEvent]
    /// Pass this back as `from:` to continue.
    public let next: OutputCursor
    /// `true` when output existed before this page and is **gone** — the
    /// harness publishes a fixed-size tail (`output_tail`), so a run that talks
    /// faster than you read loses its earlier lines. Truncation used to be
    /// silent; "this agent said little" is now distinguishable from "I am
    /// seeing the last page".
    public let truncatedBefore: Bool
    public let reachedEnd: Bool

    public init(events: [AgentEvent], next: OutputCursor,
                truncatedBefore: Bool, reachedEnd: Bool) {
        self.events = events
        self.next = next
        self.truncatedBefore = truncatedBefore
        self.reachedEnd = reachedEnd
    }
}

/// Agent output as typed events.
///
/// **The honesty rule, and it is the point of this type.** The Spaces harness
/// publishes one thing about a run's output: `output_tail`, a fixed window of
/// terminal scrollback. From that, the only cases the SDK will ever produce
/// unqualified are `.text`, `.raw` and `.finished`, plus `.stateChanged`, which
/// comes from the harness's own published status. Those carry
/// `isInferred == false`.
///
/// Everything richer — `.toolUse`, `.question`, `.artifact` — is a **guess made
/// by reading scrollback**, and is produced only when the caller has explicitly
/// imported `CuaSpacesTranscript` and installed a classifier. Those carry
/// `isInferred == true`, and a UI may then choose to render a plain bubble
/// rather than a card with buttons.
///
/// The SDK will never mint a `.question` or a `.toolUse` the agent did not
/// offer. One shared guess, opted into, labelled — instead of nine private ones
/// that each look like a fact.
public struct AgentEvent: Sendable, Hashable, Identifiable {
    public let id: UUID
    public let run: RunID
    public let cursor: OutputCursor
    public let turn: Turn.ID?
    public let at: Date?
    public let kind: Kind
    /// `true` when a classifier guessed this event out of terminal scrollback
    /// rather than the harness having published it.
    public let isInferred: Bool

    public enum Kind: Sendable, Hashable {
        /// A line of output. ANSI already stripped, on unicode **scalars** —
        /// `\r\n` is two scalars, not one `Character`.
        case text(String)
        /// A line the SDK could not say anything more about than that it
        /// arrived. Present so the SDK never has to guess to keep going.
        case raw(String)
        /// Published by the harness, never inferred.
        case stateChanged(AgentState, reason: String)
        case finished(exitCode: Int?)
        // --- inferred-only below: never produced without a classifier ---
        case toolUse(name: String, summary: String)
        case artifact(RemoteFile)
        case question(String)
        case turnEnded(Turn.ID)
    }

    public init(id: UUID = UUID(), run: RunID, cursor: OutputCursor, turn: Turn.ID? = nil,
                at: Date? = nil, kind: Kind, isInferred: Bool) {
        self.id = id
        self.run = run
        self.cursor = cursor
        self.turn = turn
        self.at = at
        self.kind = kind
        self.isInferred = isInferred
    }

    /// The plain-text projection of any event, always populated, so a caller
    /// that only wants a boring transcript never switches on `kind`.
    public var text: String {
        switch kind {
        case let .text(s), let .raw(s), let .question(s): return s
        case let .stateChanged(state, reason): return "[\(state.rawValue)] \(reason)"
        case let .finished(code): return code.map { "[exited \($0)]" } ?? "[finished]"
        case let .toolUse(name, summary): return summary.isEmpty ? name : "\(name): \(summary)"
        case let .artifact(file): return file.path
        case .turnEnded: return ""
        }
    }
}

/// One turn of the conversation.
///
/// A turn is bounded by the SDK's own record of the cursor at the moment a
/// message was delivered, so the boundary does not depend on the app having
/// been watching the tail when it happened.
public struct Turn: Sendable, Hashable, Identifiable {
    public struct ID: SpacesIdentifier {
        public let rawValue: String
        public init(_ rawValue: String) { self.rawValue = rawValue }
    }

    public let id: ID
    public let run: RunID
    /// `nil` for the turn the run was started with.
    public let message: String?
    public let startedAt: Date?
    public let began: OutputCursor
    public let ended: OutputCursor?
    public let delivery: Delivery?
    /// `true` when this turn's output has fallen out of the harness's window,
    /// so slicing the transcript at `began` would be a lie.
    public let outputLost: Bool

    public init(id: ID, run: RunID, message: String?, startedAt: Date?,
                began: OutputCursor, ended: OutputCursor?, delivery: Delivery?,
                outputLost: Bool) {
        self.id = id
        self.run = run
        self.message = message
        self.startedAt = startedAt
        self.began = began
        self.ended = ended
        self.delivery = delivery
        self.outputLost = outputLost
    }
}

// MARK: - The classifier seam

/// The seam an opt-in transcript parser plugs into.
///
/// `CuaSpaces` itself ships **no** conforming type. `CuaSpacesTranscript` ships
/// one, and importing it is the act of consent: from then on, richer events
/// appear and every one of them says `isInferred == true`.
public protocol AgentTranscriptClassifying: Sendable {
    /// Return `nil` to leave the line as plain `.text`. A returned kind is
    /// always published with `isInferred == true`.
    func classify(line: String, in run: RunID) -> AgentEvent.Kind?
}

// MARK: - Assembling a transcript from a sliding window

/// Successive `output_tail` windows, assembled into one transcript, with loss
/// detected rather than papered over.
struct TranscriptWindow: Sendable {
    private(set) var lines: [String] = []
    /// `true` once a window has slid past something we never saw.
    private(set) var lostBefore = false

    /// Ingest a tail and report which lines are new.
    @discardableResult
    mutating func ingest(_ tail: String?) -> Range<Int> {
        guard let tail, !tail.isEmpty else { return lines.count..<lines.count }
        let incoming = TranscriptWindow.split(tail)
        guard !incoming.isEmpty else { return lines.count..<lines.count }
        guard !lines.isEmpty else {
            let start = 0
            lines = incoming
            return start..<lines.count
        }
        // The largest suffix of what we have that is a prefix of what arrived.
        let maximum = min(lines.count, incoming.count)
        var overlap = 0
        for k in stride(from: maximum, through: 1, by: -1)
        where Array(lines.suffix(k)) == Array(incoming.prefix(k)) {
            overlap = k
            break
        }
        if overlap == 0 {
            // No overlap at all: the harness window moved past everything we
            // had. Lines were produced that nobody will ever see. Say so.
            lostBefore = true
        }
        let start = lines.count
        lines.append(contentsOf: incoming.dropFirst(overlap))
        return start..<lines.count
    }

    static func split(_ text: String) -> [String] {
        stripANSI(text).split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
    }

    /// ANSI stripped on unicode **scalars**. Doing it on `Character` merges
    /// `\r\n` into one grapheme and eats a line boundary.
    static func stripANSI(_ text: String) -> String {
        var out = String.UnicodeScalarView()
        var scalars = Array(text.unicodeScalars)
        var i = 0
        while i < scalars.count {
            let s = scalars[i]
            if s == "\u{1B}" {
                i += 1
                if i < scalars.count, scalars[i] == "[" || scalars[i] == "]" {
                    let isOSC = scalars[i] == "]"
                    i += 1
                    while i < scalars.count {
                        let c = scalars[i]
                        if isOSC {
                            if c == "\u{07}" { i += 1; break }
                            if c == "\u{1B}" { i += 2; break }
                        } else if (0x40...0x7E).contains(Int(c.value)) {
                            i += 1
                            break
                        }
                        i += 1
                    }
                } else if i < scalars.count {
                    i += 1
                }
                continue
            }
            if s == "\r" {
                // A bare carriage return is a terminal redraw, not a line.
                if i + 1 < scalars.count, scalars[i + 1] == "\n" { i += 1; continue }
                i += 1
                continue
            }
            out.append(s)
            i += 1
        }
        scalars = []
        return String(out)
    }
}

// MARK: - AgentRun's event surface

extension AgentRun {

    /// This run's output as typed, cursor-ordered events.
    ///
    /// The transport cannot push, so the SDK still polls — but the SDK is the
    /// only thing that does, and a consumer writes `for await`. The sequence
    /// finishes when the run does.
    ///
    /// Pass `classifier:` (from `CuaSpacesTranscript`, or your own) to get
    /// richer kinds; every event it produces says `isInferred == true`.
    /// Without one, you get `.text`, `.stateChanged` and `.finished`, all of
    /// which the harness actually published.
    public func events(since cursor: OutputCursor = .beginning,
                       pollingEvery interval: Duration = .seconds(1),
                       tail: Int = 400,
                       classifier: AgentTranscriptClassifying? = nil) -> AsyncStream<AgentEvent> {
        AsyncStream { continuation in
            let task = Task {
                var window = TranscriptWindow()
                var lastState: AgentState?
                var skipUntilLive = (cursor == .live)
                while !Task.isCancelled {
                    guard let snapshot = try? await status(tail: tail) else {
                        try? await Task.sleep(for: interval)
                        continue
                    }
                    let fresh = window.ingest(snapshot.outputTail)
                    if skipUntilLive {
                        // ".live" means nothing already in the window replays.
                        skipUntilLive = false
                    } else {
                        let turn = await space.connection.currentTurn(of: id)
                        for index in fresh where index >= cursor.line {
                            let line = window.lines[index]
                            guard !line.isEmpty else { continue }
                            let position = OutputCursor(line: index)
                            if let guessed = classifier?.classify(line: line, in: id) {
                                continuation.yield(AgentEvent(
                                    run: id, cursor: position, turn: turn,
                                    kind: guessed, isInferred: true))
                            } else {
                                continuation.yield(AgentEvent(
                                    run: id, cursor: position, turn: turn,
                                    kind: .text(line), isInferred: false))
                            }
                        }
                    }
                    if snapshot.state != lastState {
                        lastState = snapshot.state
                        continuation.yield(AgentEvent(
                            run: id, cursor: OutputCursor(line: window.lines.count),
                            turn: await space.connection.currentTurn(of: id),
                            kind: .stateChanged(snapshot.state, reason: snapshot.reason),
                            isInferred: false))
                    }
                    await space.connection.recordTranscript(window, for: id)
                    if snapshot.state.hasEnded {
                        continuation.yield(AgentEvent(
                            run: id, cursor: OutputCursor(line: window.lines.count),
                            turn: nil, kind: .finished(exitCode: snapshot.exitCode),
                            isInferred: false))
                        break
                    }
                    try? await Task.sleep(for: interval)
                }
                continuation.finish()
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    /// One page of history, oldest first.
    ///
    /// `page.truncatedBefore` is `true` when the harness's window has already
    /// dropped everything earlier. The harness publishes a fixed tail, so this
    /// is a real and frequent condition, not a theoretical one.
    public func output(from cursor: OutputCursor = .beginning,
                       limit: Int = 4_000,
                       tail: Int = 4_000) async throws -> OutputPage {
        let snapshot = try await status(tail: tail)
        var window = await space.connection.transcript(of: id)
        window.ingest(snapshot.outputTail)
        await space.connection.recordTranscript(window, for: id)

        let start = min(max(cursor.line, 0), window.lines.count)
        let end = min(start + limit, window.lines.count)
        let turn = await space.connection.currentTurn(of: id)
        let events = (start..<end).compactMap { index -> AgentEvent? in
            let line = window.lines[index]
            guard !line.isEmpty else { return nil }
            return AgentEvent(run: id, cursor: OutputCursor(line: index), turn: turn,
                              kind: .text(line), isInferred: false)
        }
        return OutputPage(events: events,
                          next: OutputCursor(line: end),
                          truncatedBefore: window.lostBefore && start == 0,
                          reachedEnd: end == window.lines.count)
    }

    /// The thread, as turns.
    public func turns() async throws -> [Turn] {
        await space.connection.turns(of: id)
    }

    /// Whether a human decision at an approval seam can actually stop this
    /// agent.
    ///
    /// **It cannot**, in any shipping backend: `agent_start` runs
    /// auto-approved and the Space *is* the sandbox. Published as a constant so
    /// a product renders "not enforced in this build" from the SDK rather than
    /// hard-coding the claim, and so the day it flips every card starts telling
    /// the truth without an app release.
    public static let approvalsAreEnforced = false

    /// Present so call sites exist before the primitive does. Throws while
    /// `approvalsAreEnforced` is `false`.
    public func approve(_ decision: ApprovalDecision) async throws {
        guard AgentRun.approvalsAreEnforced else {
            throw SpacesError.notImplementedYet(
                "approvals are not enforced by any shipping Spaces backend; "
                + "AgentRun.approvalsAreEnforced is false")
        }
    }
}
