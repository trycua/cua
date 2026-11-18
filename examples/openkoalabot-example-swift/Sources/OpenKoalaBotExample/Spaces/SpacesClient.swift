// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation

/// What OpenKoalaBots needs from Cua Spaces.
///
/// The app model is **one persistent VM per user**; each Bot gets its own *screen*
/// inside it, and screens are not separate security boundaries. The nearest Spaces
/// object is therefore a single Space created for the user, with each Bot's work
/// happening in its own window inside that one Space — *not* a Space per Bot and
/// certainly not a Space per
/// thread.
///
/// This protocol is the whole surface the rest of the app is allowed to see.
/// It is deliberately shaped the way an app wants to consume Spaces, not the
/// way the MCP happens to expose it; every gap between the two is recorded in
/// `FRICTION.md`, which is the design input for the eventual Spaces SDK.
protocol SpacesClient: AnyObject {

    // MARK: Sandboxes

    /// Every Space in the account. Maps to `list_spaces`.
    func listSpaces() async throws -> [SpaceSummary]

    /// Reuse (or create) the user's single persistent Space in their default
    /// location. Maps to `create_space` with `reuse: true`, unless a Space is
    /// pinned — see `SDKSpacesClient.spaceOverrideVariable`.
    func ensureSpace() async throws -> String

    /// Create a brand-new Space where `on` says. Maps to `create_space`.
    /// `on: .cloud` is metered: prefer `ensureSpace()`.
    func createSpace(on: SpacePlacement, image: String?, wait: Bool) async throws -> SpaceSummary

    /// Delete a Space. Maps to `delete_space`. Irreversible for a Space the
    /// app created; one added by address is only forgotten.
    func deleteSpace(_ space: String) async throws

    // MARK: Agent threads

    /// Start a Bot. Maps to `agent_start`.
    func startBot(space: String, bot: Bot, prompt: String) async throws -> AgentRun

    /// Speak to a Bot. Maps to `agent_message`.
    ///
    /// Every wired harness is single-shot, so a follow-up is a *new turn that
    /// resumes the same session* and cannot interrupt a turn already in flight.
    /// Sending to a `running` Bot is refused rather than silently queued; pass
    /// `force: true` to deliberately abandon the turn in flight.
    @discardableResult
    func message(space: String, runID: String, text: String, force: Bool) async throws -> MessageOutcome

    /// Poll a Bot. Maps to `agent_status`. Never disturbs the run.
    func status(space: String, runID: String, tail: Int) async throws -> AgentStatus

    /// A Bot's structured events after `cursor`, as the `agent_events` page
    /// JSON (`events`, `cursor`, `caught_up`). The transcript is folded from
    /// these by the SDK (`AgentTranscript`), never parsed from output text.
    func events(space: String, runID: String, cursor: UInt64) async throws -> String

    /// Stop a Bot. Maps to `agent_stop`. The result says whether it *actually*
    /// died — the tool does not assume it did.
    @discardableResult
    func stopBot(space: String, runID: String) async throws -> StopOutcome

    /// Every Bot run in the Space — the roster's backing data.
    /// Maps to `agent_list`.
    func listBots(space: String) async throws -> [AgentRunSummary]

    // MARK: Windows

    /// The windows backing each Bot's "screen". Maps to `list_space_windows`.
    func windows(space: String) async throws -> [SpaceWindow]

    // MARK: Files

    /// Push a user attachment into the Space. Maps to `upload`.
    func upload(space: String, localPath: String, remotePath: String) async throws

    /// Bring a file the Bot produced back out. Maps to `download`.
    /// Returns the host path it landed at.
    @discardableResult
    func download(space: String, remotePath: String, localDirectory: String?) async throws -> String

    // MARK: Presentation

    /// Human-facing display of a Bot's screen. See `capabilities`.
    func presentScreen(space: String, window: SpaceWindow, tier: ComputerTier) async throws

    var capabilities: SpacesCapabilities { get }
}

extension SpacesClient {
    func createSpace(on: SpacePlacement) async throws -> SpaceSummary {
        try await createSpace(on: on, image: nil, wait: true)
    }
    @discardableResult
    func message(space: String, runID: String, text: String) async throws -> MessageOutcome {
        try await message(space: space, runID: runID, text: text, force: false)
    }
    func status(space: String, runID: String) async throws -> AgentStatus {
        try await status(space: space, runID: runID, tail: 80)
    }
    @discardableResult
    func download(space: String, remotePath: String) async throws -> String {
        try await download(space: space, remotePath: remotePath, localDirectory: nil)
    }
}

// MARK: - Types

struct SpaceSummary: Identifiable, Hashable {
    var id: String
    /// Where it runs: `local`, `cloud`, `direct` or `relay`.
    var provider: String
    var os: String
    /// `running` / `Bound` for a usable Space; `Failed`, `Pending`, … otherwise.
    var phase: String
    var ip: String?

    /// Whether this Space is ready to be worked in. The two providers spell
    /// "ready" differently, which is a wart the SDK should hide.
    /// `ready` from the cua SDK; `running`/`bound` from older backends.
    var isReady: Bool { ["running", "bound", "ready"].contains(phase.lowercased()) }
}

/// A started Bot: the run id plus what its harness has *published* it can do,
/// so the app never has to infer the turn model from behaviour.
struct AgentRun {
    var runID: String
    var agent: String
    var space: String
    /// e.g. `single_shot` / `interactive`, and whether follow-ups are accepted.
    var capabilities: [String: String]
    var notes: [String]
}

/// The status vocabulary the Spaces agent harness publishes. `unknown` means
/// exactly that — a probe that failed or a signal too ambiguous to call — and
/// is never a stand-in for a guess.
enum AgentState: String, Hashable {
    case running
    case awaitingInput = "awaiting_input"
    case idle
    case finished
    case failed
    case crashed
    case unknown
}

struct AgentStatus {
    var state: AgentState
    /// Why it is in that state, in the harness's own words.
    var reason: String
    /// Whether `message(space:runID:text:)` will be accepted right now.
    var acceptsMessage: Bool
    /// The exit code of the last turn, when the turn has ended.
    var exitCode: Int?
    /// One line naming what the Bot was asked to do.
    var summary: String
    /// Recent terminal output — what the transcript shows.
    var tail: String
}

/// One row of `agent_list`: enough for the roster without a status call per Bot.
struct AgentRunSummary: Identifiable, Hashable {
    var id: String
    var agent: String
    var state: AgentState
    var summary: String
    var acceptsMessage: Bool
    var createdAt: Double?
}

struct MessageOutcome {
    /// False when the harness refused because a turn is already in flight.
    var accepted: Bool
    var reason: String
}

struct StopOutcome {
    /// The harness *verified* the process is gone, rather than assuming it.
    var stopped: Bool
    /// `nil` when the liveness probe itself could not run.
    var alive: Bool?
    var reason: String
}

struct SpaceWindow: Identifiable, Hashable {
    /// The rcdp target id, e.g. `target-172aad9a-…`. This is what a stream is
    /// opened against.
    var id: String
    var app: String
    var title: String
    var width: Int = 0
    var height: Int = 0
    var visible: Bool = true
}

/// Which parts of the Agent Computer are actually live in this build.
struct SpacesCapabilities {
    var spaceLifecycle: Bool
    var agents: Bool
    var windowList: Bool
    /// Tier 2/3 pixels. `rcdp` window streaming exists in the MCP server, but
    /// the only surfaces that *display* it (`show_space_pip`, `open_space_viewer`,
    /// `stream_space_window`) open windows on the operator's own Mac, which this
    /// build is forbidden to do. Owned by the streaming layer, not this one.
    var liveScreenPixels: Bool
    var upload: Bool
    var download: Bool
    var notes: [String]
}

// MARK: - Demo client

/// Deterministic offline client. Used for screenshot export and for the
/// `--demo` run mode so the UI can be exercised with no Space created.
final class DemoSpacesClient: SpacesClient {
    var capabilities = SpacesCapabilities(
        spaceLifecycle: false, agents: false, windowList: false, liveScreenPixels: false,
        upload: false, download: false,
        notes: ["Demo client: every Spaces call is scripted in-process. No Space is created."])

    private let window = SpaceWindow(id: "target-demo", app: "Chrome", title: "crm.acme.com")

    func listSpaces() async throws -> [SpaceSummary] {
        [SpaceSummary(id: "demo:openkoalabots", provider: "demo", os: "macos", phase: "running", ip: nil)]
    }
    func ensureSpace() async throws -> String { "demo:openkoalabots" }
    func createSpace(on: SpacePlacement, image: String?, wait: Bool) async throws -> SpaceSummary {
        try await listSpaces()[0]
    }
    func deleteSpace(_ space: String) async throws {}
    func startBot(space: String, bot: Bot, prompt: String) async throws -> AgentRun {
        AgentRun(runID: "run-\(bot.id)", agent: "claude-code", space: space,
                 capabilities: ["turn_model": "single_shot"], notes: [])
    }
    func message(space: String, runID: String, text: String, force: Bool) async throws -> MessageOutcome {
        MessageOutcome(accepted: true, reason: "demo")
    }
    func status(space: String, runID: String, tail: Int) async throws -> AgentStatus {
        AgentStatus(state: .running, reason: "demo", acceptsMessage: false,
                    exitCode: nil, summary: "demo run", tail: "")
    }
    func events(space: String, runID: String, cursor: UInt64) async throws -> String {
        #"{"events":[],"cursor":\#(cursor),"caught_up":true}"#
    }
    func stopBot(space: String, runID: String) async throws -> StopOutcome {
        StopOutcome(stopped: true, alive: false, reason: "demo")
    }
    func listBots(space: String) async throws -> [AgentRunSummary] { [] }
    func windows(space: String) async throws -> [SpaceWindow] { [window] }
    func upload(space: String, localPath: String, remotePath: String) async throws {}
    func download(space: String, remotePath: String, localDirectory: String?) async throws -> String {
        (localDirectory ?? NSTemporaryDirectory()) + "/" + (remotePath as NSString).lastPathComponent
    }
    func presentScreen(space: String, window: SpaceWindow, tier: ComputerTier) async throws {}
}
