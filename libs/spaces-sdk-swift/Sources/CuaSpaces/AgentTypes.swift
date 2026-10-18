import Foundation

/// The status vocabulary the Spaces agent harness publishes, preserved
/// verbatim.
///
/// `FRICTION.md` §9 records this as a thing the MCP got **right** and the SDK
/// must not lose: `unknown` means "the probe failed or the signal was
/// ambiguous" and is never a stand-in for a guess. It is deliberately not
/// collapsed into a smaller enum, and `RunSnapshot.reason` always travels with
/// it.
public enum AgentState: String, Sendable, Hashable, Codable, CaseIterable {
    case running
    case awaitingInput = "awaiting_input"
    case idle
    case finished
    case failed
    case crashed
    case unknown

    /// Whether the run is still doing work.
    public var isLive: Bool {
        self == .running || self == .awaitingInput || self == .idle
    }

    /// Whether the run has ended, one way or another.
    public var hasEnded: Bool {
        self == .finished || self == .failed || self == .crashed
    }

    public init(wire: String?) {
        self = AgentState(rawValue: wire ?? "") ?? .unknown
    }
}

/// **One** state type, returned in full by every call that reports state.
///
/// `FRICTION.md` §22: *"`agent_list` gives `status` and `accepts_message` but
/// no `reason`; `agent_status` gives both, plus `exit_code` and the output
/// tail. A UI that shows a state chip in the roster and the same chip in the
/// thread header therefore has two different-quality sources for one fact, and
/// a refresh of the cheap one will blank the explanation the expensive one
/// supplied."*
///
/// The SDK closes that by construction: the cheap call may omit the output
/// tail — `outputTail` is `Optional` and says so — but it never omits the
/// explanation. When the roster feed carries no `reason`, the SDK supplies the
/// last one it saw for that run rather than publishing a blank
/// (`reasonIsCarriedForward` says when it did).
public struct RunSnapshot: Sendable, Hashable, Identifiable {
    public let id: RunID
    public let space: SpaceID
    public let agent: String
    public let state: AgentState
    /// Why it is in that state, in the harness's own words. Never empty when
    /// anything has ever explained this run.
    public let reason: String
    /// Whether `AgentRun.send` will be accepted right now. **Published, not
    /// inferred** — `FRICTION.md` §9.
    public let acceptsMessage: Bool
    /// The exit code of the last turn, when the turn has ended.
    public let exitCode: Int?
    /// One line naming what the run was asked to do.
    public let summary: String
    /// Recent output. `nil` — not `""` — when this snapshot came from a call
    /// that does not carry output, so a consumer can tell "no output" from
    /// "not asked for".
    public let outputTail: String?
    /// Whether `outputTail` is a window onto a longer history. `FRICTION.md`
    /// §23: truncation used to be silent, and an app could not tell "this Bot
    /// said little" from "I am only being shown the last page".
    public let outputTruncated: Bool
    public let createdAt: Date?
    /// True when `reason` came from an earlier, richer snapshot because this
    /// call did not carry one. Surfaced rather than hidden.
    public let reasonIsCarriedForward: Bool
    /// The prompt as the harness echoes it, metadata marker and all.
    ///
    /// `summary` is the readable projection and has already been stripped;
    /// this is the carrier, and it is what lets a relaunched app recover the
    /// metadata a run was started with. `nil` when the call did not carry one.
    public let rawPrompt: String?

    public init(id: RunID, space: SpaceID, agent: String, state: AgentState, reason: String,
                acceptsMessage: Bool, exitCode: Int?, summary: String,
                outputTail: String?, outputTruncated: Bool = false,
                createdAt: Date? = nil, reasonIsCarriedForward: Bool = false,
                rawPrompt: String? = nil) {
        self.rawPrompt = rawPrompt
        self.id = id
        self.space = space
        self.agent = agent
        self.state = state
        self.reason = reason
        self.acceptsMessage = acceptsMessage
        self.exitCode = exitCode
        self.summary = summary
        self.outputTail = outputTail
        self.outputTruncated = outputTruncated
        self.createdAt = createdAt
        self.reasonIsCarriedForward = reasonIsCarriedForward
    }

    /// Anything a consumer would redraw for. Used by the roster stream to
    /// decide what changed without diffing output length.
    public func differsVisibly(from other: RunSnapshot) -> Bool {
        state != other.state || reason != other.reason
            || acceptsMessage != other.acceptsMessage || exitCode != other.exitCode
            || summary != other.summary || outputTail != other.outputTail
    }

    static func decode(_ d: [String: JSONValue], id: RunID, space: SpaceID,
                       requestedTail: Int?, carryingReasonFrom previous: RunSnapshot?) -> RunSnapshot {
        let tail = d["output_tail"]?.stringValue
        var reason = d["reason"]?.stringValue ?? ""
        var carried = false
        if reason.isEmpty, let previous, !previous.reason.isEmpty {
            reason = previous.reason
            carried = true
        }
        let truncated: Bool = {
            if let explicit = d["truncated"]?.boolValue { return explicit }
            guard let tail, let requestedTail else { return false }
            // The server returns at most `tail` lines and says nothing when it
            // clipped. A full window is the only signal available.
            return tail.split(separator: "\n", omittingEmptySubsequences: false).count >= requestedTail
        }()
        return RunSnapshot(
            id: RunID(d["run_id"]?.stringValue ?? id.rawValue),
            space: space,
            agent: d["agent"]?.stringValue ?? previous?.agent ?? "",
            state: AgentState(wire: d["status"]?.stringValue),
            reason: reason,
            acceptsMessage: d["accepts_message"]?.boolValue ?? false,
            exitCode: d["exit_code"]?.intValue,
            summary: d["summary"]?.stringValue ?? "",
            outputTail: tail,
            outputTruncated: truncated,
            createdAt: d["created_at"]?.doubleValue.map { Date(timeIntervalSince1970: $0) },
            reasonIsCarriedForward: carried,
            rawPrompt: d["prompt"]?.stringValue ?? d["raw_summary"]?.stringValue
                ?? previous?.rawPrompt)
    }
}

/// What the harness published about how this run takes turns — read from
/// `agent_start`'s `capabilities`, never guessed from behaviour
/// (`FRICTION.md` §9).
public struct TurnModel: Sendable, Hashable {
    /// e.g. `single_shot`, `interactive`.
    public let kind: String
    /// Whether follow-ups are accepted at all by this harness.
    public let acceptsFollowUps: Bool
    /// Everything the harness published, unabridged.
    public let published: [String: String]

    public init(kind: String, acceptsFollowUps: Bool, published: [String: String]) {
        self.kind = kind
        self.acceptsFollowUps = acceptsFollowUps
        self.published = published
    }

    init(_ raw: [String: JSONValue]) {
        var flat: [String: String] = [:]
        for (k, v) in raw { flat[k] = v.stringValue ?? v.description }
        self.init(kind: flat["turn_model"] ?? flat["kind"] ?? "unknown",
                  acceptsFollowUps: raw["accepts_followups"]?.boolValue
                      ?? raw["accepts_message"]?.boolValue ?? true,
                  published: flat)
    }
}

/// How a message should be delivered to a run that may be mid-turn.
///
/// `FRICTION.md` §9 keeps refusal as the default — *"Refusal beats silent
/// damage"* — and §24 records that refusal was the **only** option, so every
/// app built its own outbox. `.queueUntilIdle` is that outbox, moved into the
/// SDK: the message is visible and cancellable, nothing is dropped, and a
/// turn in flight is never killed to make room for it.
public enum DeliveryMode: Sendable, Hashable {
    /// Refuse if a turn is in flight, with the harness's explanation. Default,
    /// and it **stays** the default: *"Keep refusal as the default."*
    case refuseIfBusy

    /// Abandon the turn in flight and deliver into the wreckage.
    ///
    /// Named for what the backend does. `force: true` reaches `runner.kill`:
    /// it does not "interrupt" in the sense of cutting in politely and letting
    /// the agent resume — the turn is killed and its work is lost.
    case abandonCurrentTurn

    /// Hold the message until the run accepts it, then deliver.
    ///
    /// `queuedUntil` is not decoration. **There is no backend queue**: the
    /// message is held in this process, so it dies with this process. A
    /// product that draws a queued message as though the server had it is
    /// lying. While it waits it is visible and cancellable on `AgentRun.outbox`.
    case queue(timeout: Duration, queuedUntil: QueueLifetime)

    /// Previous spelling of `.queue(timeout:queuedUntil:.processExit)`.
    @available(*, deprecated, message: "use .queue(timeout:queuedUntil:), which declares the queue's lifetime")
    public static func queueUntilIdle(timeout: Duration) -> DeliveryMode {
        .queue(timeout: timeout, queuedUntil: .processExit)
    }

    /// Previous spelling of `.abandonCurrentTurn`, renamed because "interrupt"
    /// described something the backend does not do.
    @available(*, deprecated, renamed: "abandonCurrentTurn")
    public static var interruptCurrentTurn: DeliveryMode { .abandonCurrentTurn }
}

/// How long a queued message survives. One case, because one is the truth.
public enum QueueLifetime: Sendable, Hashable, CustomStringConvertible {
    /// Until this process exits. No Spaces backend has a message queue, so
    /// nothing outlives the client that is holding the text.
    case processExit

    public var description: String { "until this process exits" }
}

/// A message waiting to be delivered, held in this process.
public struct QueuedMessage: Sendable, Hashable, Identifiable {
    public let id: UUID
    public let run: RunID
    public let text: String
    public let queuedAt: Date
    public let queuedUntil: QueueLifetime

    public init(id: UUID = UUID(), run: RunID, text: String,
                queuedAt: Date = Date(), queuedUntil: QueueLifetime = .processExit) {
        self.id = id
        self.run = run
        self.text = text
        self.queuedAt = queuedAt
        self.queuedUntil = queuedUntil
    }
}

/// The outbox: everything this process is holding for a run, visible and
/// cancellable. It is per-process by construction — see `QueueLifetime`.
public actor Outbox {
    private var pending: [RunID: [QueuedMessage]] = [:]
    private var cancelled: Set<UUID> = []

    public init() {}

    public func messages(for run: RunID) -> [QueuedMessage] { pending[run] ?? [] }

    func enqueue(_ message: QueuedMessage) {
        pending[message.run, default: []].append(message)
    }

    func dequeue(_ message: QueuedMessage) {
        pending[message.run]?.removeAll { $0.id == message.id }
        cancelled.remove(message.id)
    }

    func isCancelled(_ id: UUID) -> Bool { cancelled.contains(id) }

    /// Withdraw a queued message before it is delivered.
    public func cancel(_ id: QueuedMessage.ID) {
        cancelled.insert(id)
        for run in pending.keys { pending[run]?.removeAll { $0.id == id } }
    }

    public func cancelAll(for run: RunID) {
        for message in pending[run] ?? [] { cancelled.insert(message.id) }
        pending[run] = []
    }
}

/// The result of a `send`, with **one** shape and a populated `reason` on both
/// branches.
///
/// `FRICTION.md` §4: *"delivered: `{delivered, run_id, note}`; refused:
/// `{delivered, run_id, status, reason}` … 'Why' is `note` in one case and
/// `reason` in the other, so it cannot be read uniformly."*
///
/// A refusal is a legitimate outcome, not an error, so it is a value — but it
/// is a value that cannot be mistaken for a delivery, because `accepted` is
/// the only discriminator and it is always present.
public struct Delivery: Sendable, Hashable {
    public let runID: RunID
    /// False when the harness refused because a turn is already in flight.
    public let accepted: Bool
    /// Why, in the harness's words. Populated whichever branch was taken.
    public let reason: String
    /// The run's state at the moment of refusal, when the harness reported it.
    public let stateAtRefusal: AgentState?
    /// How long the SDK waited before delivering, for `.queueUntilIdle`.
    public let queuedFor: Duration?

    public init(runID: RunID, accepted: Bool, reason: String,
                stateAtRefusal: AgentState? = nil, queuedFor: Duration? = nil) {
        self.runID = runID
        self.accepted = accepted
        self.reason = reason
        self.stateAtRefusal = stateAtRefusal
        self.queuedFor = queuedFor
    }

    /// For callers who want a refusal on the error channel instead.
    @discardableResult
    public func required() throws -> Delivery {
        guard accepted else {
            throw SpacesError.toolFailed(tool: "agent_message", message: reason)
        }
        return self
    }

    init(_ d: [String: JSONValue], runID: RunID, queuedFor: Duration?) {
        let delivered = d["delivered"]?.boolValue ?? false
        let why = (delivered ? d["note"] : d["reason"])?.stringValue
        self.init(runID: RunID(d["run_id"]?.stringValue ?? runID.rawValue),
                  accepted: delivered,
                  reason: why ?? (delivered ? "delivered" : "refused"),
                  stateAtRefusal: delivered ? nil : AgentState(wire: d["status"]?.stringValue),
                  queuedFor: queuedFor)
    }
}

/// The result of a `stop`, which **verifies** rather than assuming
/// (`FRICTION.md` §9).
/// One page of `agent_events`, as the server sent it.
public struct RunEventPage: Sendable, Hashable {
    /// Each event (`seq`, `ts_ms`, `turn`, `kind`, and `text`, `tool_*`,
    /// `stop_reason` when present), unabridged.
    public let events: [[String: JSONValue]]
    /// Pass back to `AgentRun.eventPage(after:)` to continue.
    public let cursor: UInt64
    /// Nothing more is written yet.
    public let caughtUp: Bool
    public let status: String

    init(_ d: [String: JSONValue]) {
        events = (d["events"]?.arrayValue ?? []).compactMap(\.objectValue)
        cursor = UInt64(max(0, d["cursor"]?.doubleValue ?? 0))
        caughtUp = d["caught_up"]?.boolValue ?? false
        status = d["status"]?.stringValue ?? ""
    }
}

public struct StopOutcome: Sendable, Hashable {
    /// The harness's verdict after an actual liveness probe.
    public let stopped: Bool
    /// `nil` when the liveness probe itself could not run — which is not the
    /// same as "alive" and is not flattened into it.
    public let alive: Bool?
    public let reason: String

    public init(stopped: Bool, alive: Bool?, reason: String) {
        self.stopped = stopped
        self.alive = alive
        self.reason = reason
    }

    init(_ d: [String: JSONValue]) {
        self.init(stopped: d["stopped"]?.boolValue ?? false,
                  alive: d["alive"]?.boolValue,
                  reason: d["reason"]?.stringValue ?? "")
    }
}

/// What a run leaves behind, and whether `AgentRun.delete()` got all of it.
///
/// `FRICTION.md` §8: *"An agent run leaves four things behind: the process, the
/// run directory, a LaunchAgent plist, and a Terminal window … `agent_stop`
/// handles the first only. A suite that must leave a demo Space pristine has to
/// know all four by path."*
public struct RunCleanup: Sendable, Hashable {
    public let runID: RunID
    public let processStopped: Bool
    public let directoryRemoved: Bool
    public let launchAgentRemoved: Bool
    public let terminalWindowClosed: Bool
    /// Anything the SDK could not remove, named rather than swallowed.
    public let residue: [String]

    public var isComplete: Bool { residue.isEmpty }

    public init(runID: RunID, processStopped: Bool, directoryRemoved: Bool,
                launchAgentRemoved: Bool, terminalWindowClosed: Bool, residue: [String]) {
        self.runID = runID
        self.processStopped = processStopped
        self.directoryRemoved = directoryRemoved
        self.launchAgentRemoved = launchAgentRemoved
        self.terminalWindowClosed = terminalWindowClosed
        self.residue = residue
    }
}

/// What to start.
///
/// `metadata` is the field `FRICTION.md` §21 and §41 ask for. The harness has
/// nowhere to put an application's own key, so both OpenKoalaBots's roster
/// identity and its routine attribution had to be smuggled into the prompt,
/// contaminating the text the agent reads and forcing every display path to
/// strip it again. Until the server carries metadata, the SDK does the
/// smuggling in **one** place, out of the prompt body, and strips it back out
/// of every `summary` it publishes.
public struct AgentStartRequest: Sendable, Hashable {
    public var agent: String
    public var prompt: String
    public var metadata: [String: String]

    /// Whether the run gets a visible terminal window on the Space's desktop.
    ///
    /// On by default: for a person using a Space with a screen, watching the
    /// agent work is the point. **A test suite should set it `false`.** A suite
    /// that starts dozens of runs otherwise leaves dozens of terminal windows
    /// on the machine, and the cheapest cleanup to get right is the one where
    /// nothing was created in the first place — the surviving windows are what
    /// trashed a demo machine twice. Maps to `agent_start`'s `show`, which
    /// likewise defaults true server-side, so an older server that does not
    /// know the key behaves as it always did.
    ///
    /// See `samples/openkoalabots/FRICTION.md` §54.
    public var showsWindow: Bool

    /// How long the **server** should let this run live.
    ///
    /// Reserved, and reserved deliberately. No Spaces backend implements it —
    /// `ProviderCapabilities.serverBackstop` is `false` everywhere — so today
    /// the key is sent and ignored. It exists now because every comparable
    /// sandbox product makes the server the backstop and we have none: a
    /// `SIGKILL`ed client never runs its `defer`, and the residue is orphaned
    /// processes and windows on someone's machine. Reserving the field now
    /// means gaining the behaviour later is not a breaking change.
    public var timeout: Duration?

    /// A custom model endpoint for the run (`agent_start`'s `base_url`,
    /// `model` and `env_from_host`). `nil`: the harness's own default.
    public var endpoint: AgentEndpoint?

    public init(agent: String = "claude-code", prompt: String,
                metadata: [String: String] = [:], showsWindow: Bool = true,
                timeout: Duration? = nil, endpoint: AgentEndpoint? = nil) {
        self.timeout = timeout
        self.endpoint = endpoint
        self.agent = agent
        self.prompt = prompt
        self.metadata = metadata
        self.showsWindow = showsWindow
    }
}

/// Where a run's model lives, when it is not the harness default.
///
/// The key never travels in this value: `envFromHost` names provider key
/// variables (for example `ANTHROPIC_API_KEY`) that the Spaces host forwards
/// from its own environment to the run.
public struct AgentEndpoint: Sendable, Hashable {
    public var baseURL: String
    public var model: String?
    public var envFromHost: [String]

    public init(baseURL: String, model: String? = nil, envFromHost: [String] = []) {
        self.baseURL = baseURL
        self.model = model
        self.envFromHost = envFromHost
    }
}

