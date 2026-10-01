// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import SwiftUI

// MARK: - Where a screen gets its data

/// The single seam between the views and their data.
///
/// Every screen is rendered from one of these: `FixtureDataSource` for the PNG
/// export and the rubric, `BotStore` at runtime against a live Space. The views
/// themselves take plain `Bot` / `Thread` values, so nothing in the view layer
/// knows which side of this protocol it is being fed from, which is exactly
/// why the export path keeps working unchanged.
@MainActor
protocol BotDataSource {
    /// The roster, in the order the home screen should show it.
    var bots: [Bot] { get }
    /// The Bot's one long-lived thread.
    func thread(for botID: String) -> Thread
    /// What the Bot is doing right now, and whether it will take a message.
    func presence(for botID: String) -> BotPresence
}

// MARK: - Live per-Bot state

/// A Bot's live state, modelled on the harness's own published vocabulary
/// rather than on strings parsed out of its output.
///
/// `acceptsMessage` is carried separately from `state` on purpose: the harness
/// publishes it, so the UI can *show* that a message will be refused instead of
/// letting the user find out by having one bounce. See `FRICTION.md` #9: this
/// is the thing the SDK must not lose.
struct BotPresence: Hashable {
    /// The agent thread backing this Bot. `nil` when the Bot has no thread.
    ///
    /// In the running app this is never `nil` for a Bot the user can see:
    /// creating a Bot starts its thread in the same act, so a roster row and a
    /// live conversation come into existence together. It stays optional
    /// because the *store* has a moment between minting the identity and the
    /// `agent_start` returning, and because a run can be adopted before its
    /// status has been read.
    var runID: String?
    var state: AgentState
    /// The harness's own words for why it is in this state.
    var reason: String
    /// Whether `agent_message` will be accepted right now.
    var acceptsMessage: Bool
    /// One line naming what the Bot was asked to do.
    var summary: String
    var exitCode: Int?

    /// The state of a Bot whose thread has not been read yet. Not a user-
    /// facing condition: nothing renders a Bot in this state, because a Bot
    /// with no conversation does not get a row.
    static let unstarted = BotPresence(
        runID: nil, state: .unknown, reason: "no run started for this Bot",
        acceptsMessage: false, summary: "")

    init(runID: String? = nil, state: AgentState = .unknown, reason: String = "",
         acceptsMessage: Bool = false, summary: String = "", exitCode: Int? = nil) {
        self.runID = runID
        self.state = state
        self.reason = reason
        self.acceptsMessage = acceptsMessage
        self.summary = summary
        self.exitCode = exitCode
    }

    init(runID: String, status: AgentStatus) {
        self.init(runID: runID, state: status.state, reason: status.reason,
                  acceptsMessage: status.acceptsMessage, summary: status.summary,
                  exitCode: status.exitCode)
    }

    init(_ row: AgentRunSummary) {
        self.init(runID: row.id, state: row.state, reason: "",
                  acceptsMessage: row.acceptsMessage, summary: row.summary)
    }

    /// Whether there is an agent thread behind this Bot.
    var hasThread: Bool { runID != nil }

    /// Short human label for the status chip. Deliberately not the raw enum:
    /// `awaiting_input` means "it is waiting on *you*", which is the one state
    /// a user must not misread.
    var label: String {
        switch state {
        case .running:       return "Working"
        case .awaitingInput: return "Waiting for you"
        case .idle:          return "Idle"
        case .finished:      return exitCode.map { $0 == 0 ? "Finished" : "Finished (exit \($0))" }
                                    ?? "Finished"
        case .failed:        return "Failed"
        case .crashed:       return "Crashed"
        case .unknown:       return "Unknown"
        }
    }

    /// The colour of the state dot. `unknown` is grey, never green: an
    /// unreadable probe must not look like health.
    var tintHex: UInt32 {
        switch state {
        case .running:       return 0x8B5CF6
        case .awaitingInput: return 0xE0AE09
        case .idle:          return 0x2F80F0
        case .finished:      return (exitCode ?? 0) == 0 ? 0x18BE4B : 0xF4234B
        case .failed, .crashed: return 0xF4234B
        case .unknown:       return 0x9A9A9A
        }
    }

    /// The one-line explanation shown next to the composer when the Bot will
    /// refuse a message. `nil` when a message would go through.
    var refusalHint: String? {
        guard !acceptsMessage else { return nil }
        switch state {
        case .running:  return "Working: a message now would be refused until this turn ends."
        case .unknown:  return hasThread
            ? "Status unreadable: a message may be refused."
            : "Not connected to this Bot's thread."
        case .failed, .crashed: return "This Bot's run ended badly; it cannot take a message."
        case .finished: return "This run has finished and will not take another message."
        default:        return "Not accepting messages right now."
        }
    }
}

// MARK: - Fixtures as a data source

/// The export path's data source. Nothing here touches a Space: `RUBRIC.md`
/// treats its renders as regression baselines, so those renders must stay
/// byte-deterministic.
struct FixtureDataSource: BotDataSource {
    var bots: [Bot] = Fixtures.bots

    /// `nonisolated` so the views can name it as a *default argument*.
    /// `BotDataSource` is `@MainActor`, which makes the synthesised memberwise
    /// initialiser main-actor-isolated, and a default argument is evaluated in
    /// a nonisolated context, so `source: BotDataSource = FixtureDataSource()`
    /// would not compile without this. It holds nothing but fixtures, so there
    /// is nothing for the isolation to protect.
    nonisolated init() {}
    func thread(for botID: String) -> Thread { Fixtures.thread(for: botID) }
    /// Fixture Bots are drawn as started and idle so the chrome that shows
    /// presence has something honest to render offline.
    func presence(for botID: String) -> BotPresence {
        BotPresence(runID: "run-fixture", state: .idle,
                    reason: "fixture data: no Space attached",
                    acceptsMessage: true, summary: Fixtures.bot(botID).preview)
    }
}
