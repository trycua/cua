// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Cua
import Foundation
import SwiftUI

/// The app's live data layer: one Space, a roster of Bots, and one long-lived
/// agent thread per Bot.
///
/// **One Space, many Bots.** `SpacesClient.swift` argues this out in full: Koala
/// Bot is one persistent VM per user with a *screen* per Bot, and screens are
/// explicitly not security boundaries. So this store creates (or attaches to)
/// exactly one Space and starts one `agent_start` thread per Bot inside it. It
/// never creates a Space per Bot, and it never deletes the Space it attached
/// to.
///
/// Everything the views need comes through `BotDataSource`, so the same screens
/// render from `FixtureDataSource` during PNG export and from this store at
/// runtime.
@MainActor
final class BotStore: ObservableObject, BotDataSource {

    // MARK: Published state

    enum Connection: Equatable {
        case detached
        case connecting
        case attached(String)
        case failed(String)

        var spaceID: String? { if case .attached(let s) = self { return s }; return nil }
    }

    /// A message the user must see rather than have swallowed: a refused
    /// `agent_message`, a failed tool call. Refusals are *not* errors — the
    /// harness refusing a mid-turn message is correct behaviour — but they are
    /// also not nothing, so they surface here and in the transcript.
    struct Notice: Identifiable, Equatable {
        enum Kind: Equatable { case refusal, failure, info }
        let id = UUID()
        var kind: Kind
        var botID: String?
        var text: String
    }

    @Published private(set) var connection: Connection = .detached
    /// The roster, most recently active first, pinned Bots keep their flag.
    @Published private(set) var bots: [Bot] = []
    @Published private(set) var presences: [String: BotPresence] = [:]
    @Published private(set) var threads: [String: Thread] = [:]
    @Published private(set) var notices: [Notice] = []
    /// Windows in the Space — what the Agent Computer screens are opened against.
    @Published private(set) var spaceWindows: [SpaceWindow] = []

    // MARK: Internals

    private let client: SpacesClient
    /// Local Bot identity. The Space knows about *runs*, not about Bots: it has
    /// no notion of a name, a colour or a pinned flag, so identity lives here
    /// and is joined to the run list by marker (see `marker(for:)`).
    private var identities: [String: Bot]
    private var order: [String]
    /// botID -> run id.
    private var runs: [String: String] = [:]
    /// botID -> the turns we know about, oldest first.
    private var turns: [String: [BotTurn]] = [:]
    /// botID -> the run's events folded by the SDK into transcript items
    /// (`AgentTranscript`: agent messages, the prompt, and muted activity
    /// groups). The one classification rule every sample shares.
    private var transcripts: [String: CuaSDK.AgentTranscript] = [:]
    /// botID -> the run its transcript was folded from.
    private var transcriptRuns: [String: String] = [:]
    /// botID -> the last runner turn a message of this app became (turn 1
    /// is the prompt the run was started with).
    private var runTurns: [String: UInt32] = [:]
    /// botID -> messages this app put in the thread itself, before any agent
    /// output: the greeting a freshly created Bot opens with and the choice
    /// card under it. They are not derived from the run's tail, so
    /// `rebuildThread` has to be told about them or the next poll erases them.
    private var seeded: [String: [Message]] = [:]
    /// botID -> the answers the user has given to choice cards, oldest first.
    /// The nth card in a thread is resolved by the nth answer.
    private var choiceAnswers: [String: [String]] = [:]
    /// Bots whose choice cards the user dismissed.
    private var dismissedChoices: Set<String> = []
    /// Bots whose thread is showing a typing indicator rather than a greeting.
    @Published private(set) var typing: Set<String> = []

    /// Bots the user has spoken to and is waiting on, keyed to the runner turn
    /// their message becomes.
    ///
    /// This is the send-side half of the indicator, and it is deliberately
    /// **not** `typing`: `typing` means "this Bot has never spoken and owes a
    /// greeting", and it is cleared the moment the tail stops being empty. On
    /// a send the tail is already long, so that test can never fire, which is
    /// exactly why sending a message used to show nothing at all until the
    /// whole reply landed.
    ///
    /// The stored value is what makes the clear honest. A spinner driven by a
    /// timer, or by a bare boolean, is a spinner that can get stuck; this one
    /// is answered by the same offset arithmetic that attributes output to a
    /// turn, so "the reply has begun" is a fact about the tail rather than a
    /// guess about elapsed time.
    @Published private(set) var pending: [String: UInt32] = [:]

    /// Is this Bot owed something the user is visibly waiting for — a greeting
    /// it has not given yet, or a reply to a message just sent?
    ///
    /// The one question the transcript asks. Both halves clear on real state:
    /// see `firstOutputArrived` and `clearPending`.
    func isAwaitingReply(_ botID: String) -> Bool {
        typing.contains(botID) || pending[botID] != nil
    }

    /// Stop waiting on `botID`. Every exit from a send funnels through here —
    /// first token, turn finished, refusal, transport error, failed probe and
    /// an explicit stop — because the failure mode being fixed is a spinner
    /// with no path out of it.
    private func clearPending(_ botID: String) {
        if pending.removeValue(forKey: botID) != nil { objectWillChange.send() }
    }
    private var pollTask: Task<Void, Never>?
    private let calendar = Calendar.current

    /// One turn of a Bot's thread: what the user said, whether it was taken,
    /// and which of the run's turns it became.
    ///
    /// Every event the runner writes carries its turn number, so output is
    /// attributed to the turn that caused it by that number (`runTurn`), not
    /// by slicing a text tail. `nil` for a refused message or an attachment.
    struct BotTurn {
        var userText: String?
        var attachments: [Attachment] = []
        var refusalReason: String? = nil
        var runTurn: UInt32? = nil
        var reaction: String? = nil
        var startedAt: Date = Date()
    }

    /// Whether a run this app did not start gets a roster row.
    ///
    /// **Off.** The sidebar is a list of the user's *conversations*, and a run
    /// somebody else started in the shared Space is not one of them. It used
    /// to be on, with the reasoning that a run doing work in the user's Space
    /// which the UI cannot see is worse than an ugly row — and on the live
    /// demo Space, which has thirty-odd runs in it, that reasoning produced
    /// exactly what the user reported: a sidebar full of threads they had
    /// never opened. A Bot appears when the user creates one or installs one.
    let adoptsForeignRuns: Bool

    /// Where roster *names* are kept between launches. `nil` means "do not
    /// persist", which is the default and is what every test gets: a store that
    /// reads a file in Application Support is not hermetic, and a test that
    /// passes or fails depending on what the app did the last time somebody ran
    /// it is worse than no test. The app passes `rosterFileURL()`.
    private let rosterFile: URL?

    /// The roster this store starts with.
    ///
    /// **Empty**, and that is the whole point of this parameter existing. It
    /// used to default to `Fixtures.bots`, so launching the app showed nine
    /// coworkers nobody had hired and no conversation existed with. The
    /// fixtures are the *export path's* data (`FixtureDataSource`), and
    /// `RUBRIC.md` grades thirteen renders built from them; they are not the
    /// running app's roster and never were. Tests that want a populated store
    /// pass one explicitly.
    init(client: SpacesClient, identities roster: [Bot] = [],
         adoptsForeignRuns: Bool = false, rosterFile: URL? = nil) {
        self.client = client
        self.adoptsForeignRuns = adoptsForeignRuns
        self.rosterFile = rosterFile
        self.identities = Dictionary(uniqueKeysWithValues: roster.map { ($0.id, $0) })
        self.order = roster.map(\.id)
        self.bots = roster
    }

    deinit { pollTask?.cancel() }

    // MARK: - BotDataSource

    func thread(for botID: String) -> Thread {
        fixtureThreads[botID] ?? threads[botID] ?? Thread(botID: botID, messages: [])
    }

    /// Fixture transcripts for the design capture (`UICapture.designVariable`),
    /// shown in place of a live thread. Never set in normal use.
    @Published private(set) var fixtureThreads: [String: Thread] = [:]

    func showFixture(_ thread: Thread) { fixtureThreads[thread.botID] = thread }

    func presence(for botID: String) -> BotPresence {
        presences[botID] ?? .unstarted
    }

    func bot(_ id: String) -> Bot? { identities[id] }

    var spaceID: String? { connection.spaceID }

    func runID(for botID: String) -> String? { runs[botID] }

    // MARK: - Connecting

    /// Attach to the user's single persistent Space and read the roster back.
    ///
    /// Attaches; never deletes. With `OPENKOALABOTS_TEST_SPACE` set this is a
    /// pure attach that creates nothing at all — the only safe way to run
    /// against a Space someone demos from (`FRICTION.md` #5).
    func connect() async {
        connection = .connecting
        do {
            let space = try await client.ensureSpace()
            connection = .attached(space)
            // Names first, then the runs. The other order would let
            // `improvisedBot` mint a name for a Bot the user already named.
            loadRosterIdentities()
            await refreshRoster()
        } catch {
            connection = .failed("\(error)")
            post(.failure, nil, "could not attach to a Space: \(error)")
        }
    }

    /// Read `agent_list` and join it to local Bot identity.
    ///
    /// A run carries no Bot identity of its own, so the join is done on a
    /// marker this store writes into the prompt (and which the harness echoes
    /// back in `summary`). Runs with no marker are still shown — as Bots named
    /// after their agent — rather than hidden, because a run doing work in the
    /// user's Space that the UI cannot see is worse than an ugly row.
    func refreshRoster() async {
        guard let space = connection.spaceID else { return }
        do {
            let rows = try await client.listBots(space: space)
            apply(roster: rows)
        } catch {
            post(.failure, nil, "agent_list failed: \(error)")
        }
    }

    func apply(roster rows: [AgentRunSummary]) {
        var seen = Set<String>()
        for row in rows {
            guard let botID = Self.botID(fromSummary: row.summary)
                    ?? (adoptsForeignRuns ? adoptedID(for: row) : nil)
            else { continue }
            // A marked run whose identity this store no longer holds is a run
            // from a previous launch. Adopt it: the conversation is real and
            // the user had it. Without this the roster would empty itself on
            // every restart, which is the opposite failure to the one above.
            seen.insert(botID)
            // Another run carrying the same marker (an earlier launch's)
            // never takes over a Bot whose own run is still listed.
            if let current = runs[botID], current != row.id,
               rows.contains(where: { $0.id == current }) { continue }
            runs[botID] = row.id
            // A Bot is on the roster when its **run** is reported, never
            // because an identity for it happens to be in memory or on disk.
            // The condition is membership of `order`, not of `identities`,
            // precisely so that a saved name cannot resurrect a conversation
            // whose run is gone — which is the fixture-roster defect wearing a
            // different hat.
            if identities[botID] == nil {
                identities[botID] = Self.improvisedBot(id: botID, row: row, index: order.count)
            }
            if !order.contains(botID) {
                order.append(botID)
            }
            var p = presences[botID] ?? .unstarted
            // agent_list carries state and accepts_message but no reason; keep
            // the reason from the last agent_status rather than blanking it.
            p.runID = row.id
            p.state = row.state
            p.acceptsMessage = row.acceptsMessage
            if !row.summary.isEmpty { p.summary = Self.strippingMarker(row.summary) }
            presences[botID] = p
            if var b = identities[botID] {
                b.preview = transcripts[botID]?.preview() ?? p.summary
                if let t = row.createdAt { b.timestamp = Self.timestamp(Date(timeIntervalSince1970: t)) }
                identities[botID] = b
            }
        }
        for id in order where !seen.contains(id) {
            if presences[id] == nil { presences[id] = .unstarted }
        }
        applySavedOrdering()
        rebuildBots()
    }

    private func rebuildBots() {
        bots = order.compactMap { identities[$0] }
    }

    /// Put the rows back in the order the user last saw them.
    ///
    /// `agent_list` returns runs in the Space's order, which is not the
    /// sidebar's: a Bot created most recently sits at the top, and that
    /// position is the user's, not the harness's. Only rows that are already
    /// on the roster are touched — this sorts, it never adds.
    private func applySavedOrdering() {
        guard !savedOrder.isEmpty else { return }
        let rank = Dictionary(uniqueKeysWithValues: savedOrder.enumerated().map { ($1, $0) })
        let known = order.filter { rank[$0] != nil }.sorted { rank[$0]! < rank[$1]! }
        let unknown = order.filter { rank[$0] == nil }
        order = known + unknown
    }

    // MARK: - Hiring and steering

    /// Put a Bot on the roster before it is hired.
    ///
    /// Hiring from the UI needs this because the Space has no notion of a Bot
    /// at all: `agent_start` takes a prompt, not an identity (`FRICTION.md`
    /// §21). So a new Bot exists *here* first — name, shape, colour, roster
    /// position — and only then does `hire` start the run that gets joined back
    /// to it by marker. Registering is idempotent and never starts anything.
    func register(_ bot: Bot, atTop: Bool = false) {
        if identities[bot.id] == nil {
            if atTop { order.insert(bot.id, at: 0) } else { order.append(bot.id) }
        }
        identities[bot.id] = bot
        if presences[bot.id] == nil { presences[bot.id] = .unstarted }
        rebuildBots()
    }

    /// Create a Bot **and its conversation**, in that order, and return it.
    ///
    /// The ordering is deliberate, not a convenience: creation starts
    /// the agent first, refreshes the roster, and only then opens it, so a row
    /// never exists before a real agent does. This app used to do the reverse —
    /// mint a roster identity, show it, then try to start a run — which is how
    /// a row could exist with nothing behind it and need a word for that state.
    /// There is no such state here: if the start fails, no row appears and the
    /// caller is told why.
    ///
    /// The new Bot goes to the **top** of the list and opens immediately with a
    /// greeting, preceded by a typing indicator.
    /// Whether a Bot is still being created — its agent has been started but
    /// nothing has come back from it yet. The row shows `Creating…` meanwhile.
    @Published private(set) var creating: Set<String> = []

    @discardableResult
    func createConversation(named name: String, dark: Bool = true,
                            kickstart: Bool = true,
                            opening: String = BotStore.openingInstruction) async throws -> Bot {
        guard let space = connection.spaceID else { throw StoreError.notAttached }
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        let title = trimmed.isEmpty ? Self.newBotName : trimmed
        let id = Self.identifier(for: title, taken: Set(identities.keys))
        // Minted, not registered: nothing is on the roster yet.
        var bot = Persona.bot(id: id, name: title, dark: dark,
                              timestamp: Self.timestamp(Date()),
                              screenIndex: order.count)
        // `isKickstartRequested`: the Bot is told to speak first. This is why a
        // fresh account's single row already has a last message — the greeting
        // is a real turn the agent took, not a string the client drew.
        let prompt = kickstart
            ? "\(Self.marker(for: id)) \(opening)\n\n\(Self.kickstartInstruction)"
            : "\(Self.marker(for: id)) \(opening)"
        let run = try await client.startBot(space: space, bot: bot, prompt: prompt)
        // The agent exists. Only now does a row.
        bot.preview = ""
        runs[id] = run.runID
        transcripts[id] = CuaSDK.AgentTranscript()
        transcriptRuns[id] = run.runID
        runTurns[id] = 1
        // No turn: the opening instruction is not something the user said, so
        // it must not appear in the transcript as their message.
        turns[id] = []
        seeded[id] = []
        presences[id] = BotPresence(runID: run.runID, state: .running,
                                    reason: "just started", acceptsMessage: false,
                                    summary: "")
        register(bot, atTop: true)
        saveRosterIdentities()
        creating.insert(id)
        if kickstart { typing.insert(id) }
        rebuildThread(id)
        return bot
    }

    /// What `kickstartAwaitingFirstMessage` amounts to over this transport:
    /// the agent is asked to open the conversation, and told how to put a
    /// choice card in it if it wants one. No wording is dictated — see
    /// `AgentOutputParser.choiceCard`.
    static let kickstartInstruction =
        "Open the conversation with one short greeting. "
        + "If you want to offer the user a small set of choices, emit them as "
        + "a block: a line `[[choices: <your question>]]`, then one option per "
        + "line, then `[[/choices]]`."

    /// The default name a `Create new Bot` gives its Bot. `New Bot`, not
    /// `New chat`: the roster is of Bots, not chats.
    static let newBotName = "New Bot"

    /// What a freshly created Bot is actually told to do in the Space. The
    /// user never sees this — it is not a turn in the transcript — which is
    /// why creating a Bot shows a greeting rather than an echoed prompt.
    static let openingInstruction =
        "You are a teammate agent working in a Cua Space. "
        + "Wait for the user's first instruction."

    /// The Bot has spoken: it is no longer being created and no longer typing.
    ///
    /// Called from `refresh` the moment the tail stops being empty. Nothing
    /// here fabricates a greeting — the greeting is whatever the agent said,
    /// which is the point of asking it to speak first.
    private func firstOutputArrived(_ botID: String) {
        creating.remove(botID)
        typing.remove(botID)
    }

    /// Resolve a choice card in a Bot's thread: record the answer and send it.
    ///
    /// The resolution is stored per Bot rather than written into the message,
    /// because the message is rebuilt from the agent's tail on every poll and
    /// anything written into it would be erased a second later.
    func answer(_ text: String, to botID: String) async {
        choiceAnswers[botID, default: []].append(text)
        rebuildThread(botID)
        _ = await send(text, to: botID)
    }

    /// Dismiss a choice card without answering it.
    func dismissChoices(in botID: String) {
        dismissedChoices.insert(botID)
        rebuildThread(botID)
    }

    // MARK: - Roster identity, across launches

    /// Where the roster's *names* live.
    ///
    /// The Space stores runs, not Bots: a run has an agent, a state and a
    /// prompt, and no name, colour or shape. So when the app restarts and
    /// `agent_list` returns the user's own past conversations, the only thing
    /// joining them back to the Bots they belong to is the marker in the
    /// prompt — which gives an id but not a name. Without this file a Bot the
    /// user called `Inbox Zero` came back as `claude-code`: a real conversation
    /// wearing the wrong label, and a quieter version of the same defect this
    /// rewrite is about. The sidebar must say what the user made.
    ///
    /// Caught by photographing the running app twice, not by reading the code.
    static func rosterFileURL() -> URL {
        let dir = AppDataDirectory.url()
        return dir.appendingPathComponent("roster.json")
    }

    /// The saved ordering, used only to sort rows whose runs came back. Never
    /// used to decide *whether* a row exists.
    private var savedOrder: [String] = []

    private struct SavedRoster: Codable {
        var order: [String]
        var identities: [Bot]
    }

    /// Read the saved names back.
    ///
    /// Nothing is *shown* because of this — a Bot still appears only when
    /// `agent_list` reports its run — so a stale entry for a run that no longer
    /// exists cannot resurrect a dead conversation. It supplies the name,
    /// colour and shape for one that does.
    func loadRosterIdentities(from url: URL? = nil) {
        guard let file = url ?? rosterFile else { return }
        guard let data = try? Data(contentsOf: file),
              let saved = try? JSONDecoder().decode(SavedRoster.self, from: data)
        else { return }
        for bot in saved.identities where identities[bot.id] == nil {
            identities[bot.id] = bot
        }
        // `order` is deliberately **not** restored. It is the list of Bots the
        // sidebar shows, and that list is built from the runs `agent_list`
        // reports. Restoring it here would put a row back for every Bot the
        // user ever made, run or no run.
        savedOrder = saved.order
    }

    @discardableResult
    func saveRosterIdentities(to url: URL? = nil) -> Bool {
        guard let file = url ?? rosterFile else { return false }
        let saved = SavedRoster(order: order, identities: order.compactMap { identities[$0] })
        do {
            let encoder = JSONEncoder()
            encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
            try encoder.encode(saved).write(to: file, options: .atomic)
            return true
        } catch {
            post(.failure, nil, "could not save the roster: \(error)")
            return false
        }
    }

    /// A stable, readable id from a display name, unique within the roster.
    static func identifier(for name: String, taken: Set<String>) -> String {
        let base = name.lowercased()
            .components(separatedBy: CharacterSet.alphanumerics.inverted)
            .filter { !$0.isEmpty }
            .joined(separator: "-")
        let root = base.isEmpty ? "bot" : base
        if !taken.contains(root) { return root }
        var n = 2
        while taken.contains("\(root)-\(n)") { n += 1 }
        return "\(root)-\(n)"
    }

    /// Start a Bot's one long-lived agent thread in the shared Space.
    ///
    /// Kept for the paths that mint an identity themselves — the Routines
    /// runner and the group messenger, which both have to be able to start a
    /// Bot that is already on the roster. New conversations from the UI go
    /// through `createConversation`, which starts the agent *first*.
    @discardableResult
    func hire(_ botID: String, prompt: String) async throws -> String {
        guard let space = connection.spaceID else {
            throw StoreError.notAttached
        }
        guard let bot = identities[botID] else { throw StoreError.unknownBot(botID) }
        let marked = "\(Self.marker(for: botID)) \(prompt)"
        let run = try await client.startBot(space: space, bot: bot, prompt: marked)
        runs[botID] = run.runID
        transcripts[botID] = CuaSDK.AgentTranscript()
        transcriptRuns[botID] = run.runID
        runTurns[botID] = 1
        turns[botID] = [BotTurn(userText: prompt, runTurn: 1)]
        presences[botID] = BotPresence(runID: run.runID, state: .running,
                                       reason: "just started", acceptsMessage: false,
                                       summary: prompt)
        rebuildThread(botID)
        await refresh(botID)
        return run.runID
    }

    /// Speak to a Bot.
    ///
    /// A refusal is **not** swallowed: it comes back as `false`, lands in
    /// `notices`, and is written into the transcript as a centred system line,
    /// so the user can see that their message did not reach the Bot.
    @discardableResult
    func send(_ text: String, to botID: String) async -> MessageOutcome {
        guard let space = connection.spaceID else {
            let o = MessageOutcome(accepted: false, reason: "not attached to a Space")
            post(.failure, botID, o.reason)
            return o
        }
        guard let run = runs[botID] else {
            let o = MessageOutcome(accepted: false, reason: "this Bot has no agent thread")
            post(.failure, botID, o.reason)
            return o
        }
        // The runner numbers turns 1, 2, ...: an accepted message becomes the
        // next one. A run adopted from an earlier launch may be further on.
        let seen = transcripts[botID]?.items().map(\.turn).max() ?? 0
        let nextTurn = max(runTurns[botID] ?? 1, seen) + 1

        // The user's message goes into the transcript *before* the round trip,
        // together with the pending marker. Appending it after the `await` is
        // what made the window sit still after a send: on a Space that takes a
        // beat to answer, neither the message nor any sign of it existed until
        // the reply did. The turn is amended below if the send is refused.
        turns[botID, default: []].append(BotTurn(userText: text))
        let index = turns[botID]!.count - 1
        pending[botID] = nextTurn
        rebuildThread(botID)

        do {
            let outcome = try await client.message(space: space, runID: run, text: text)
            if !outcome.accepted {
                // A refusal is an exit, not a pause. FRICTION §24: there is no
                // outbox, so a refused message is never going to be answered
                // and must not leave a bubble waiting for an answer.
                let reason = outcome.reason.isEmpty ? "refused" : outcome.reason
                turns[botID]?[index].refusalReason = reason
                clearPending(botID)
                post(.refusal, botID, "\(name(botID)) did not take that message: \(reason)")
                rebuildThread(botID)
            } else {
                // Accepted: the message is the run's next turn. The
                // indicator stays up, and `refresh` takes it down on the
                // turn's first output or when the turn ends.
                runTurns[botID] = nextTurn
                turns[botID]?[index].runTurn = nextTurn
                await refresh(botID)
            }
            return outcome
        } catch {
            let o = MessageOutcome(accepted: false, reason: "\(error)")
            turns[botID]?[index].refusalReason = o.reason
            clearPending(botID)
            post(.failure, botID, "agent_message failed: \(error)")
            rebuildThread(botID)
            return o
        }
    }

    /// Attach a file to a Bot's thread: upload it into the shared Space and
    /// echo it in the transcript so the user can see what the Bot was given.
    @discardableResult
    func attach(_ localPath: String, to botID: String,
                remoteDirectory: String = "/tmp/openkoalabots") async -> Attachment? {
        guard let space = connection.spaceID else {
            post(.failure, botID, "not attached to a Space"); return nil
        }
        let name = (localPath as NSString).lastPathComponent
        let remote = "\(remoteDirectory)/\(name)"
        do {
            try await client.upload(space: space, localPath: localPath, remotePath: remote)
            let size = (try? FileManager.default.attributesOfItem(atPath: localPath)[.size] as? Int) ?? nil
            let a = Attachment(name: name, byteCount: size ?? 0)
            var turn = BotTurn(userText: nil, attachments: [a])
            turn.userText = nil
            turns[botID, default: []].append(turn)
            rebuildThread(botID)
            return a
        } catch {
            post(.failure, botID, "upload failed: \(error)")
            return nil
        }
    }

    /// Bring a file the Bot produced back out to the host.
    @discardableResult
    func fetch(_ remotePath: String, to directory: String? = nil) async -> String? {
        guard let space = connection.spaceID else { return nil }
        do { return try await client.download(space: space, remotePath: remotePath,
                                              localDirectory: directory) }
        catch { post(.failure, nil, "download failed: \(error)"); return nil }
    }

    /// Stop a Bot's turn. The outcome says whether it *actually* died.
    @discardableResult
    func stop(_ botID: String) async -> StopOutcome? {
        guard let space = connection.spaceID, let run = runs[botID] else { return nil }
        do {
            let out = try await client.stopBot(space: space, runID: run)
            if !out.stopped {
                post(.failure, botID, "stop did not confirm the run died: \(out.reason)")
            }
            await refresh(botID)
            return out
        } catch {
            post(.failure, botID, "agent_stop failed: \(error)")
            // The user asked it to stop and we could not even ask. Whatever is
            // true of the run, the user is no longer waiting on this turn.
            clearPending(botID)
            return nil
        }
    }

    /// Attach a reaction to a Bot's most recent turn.
    func react(_ emoji: String, to botID: String) {
        guard var list = turns[botID], !list.isEmpty else { return }
        list[list.count - 1].reaction = emoji
        turns[botID] = list
        rebuildThread(botID)
    }

    // MARK: - Polling

    /// Pull one Bot's status and rebuild its transcript tail.
    ///
    /// This is a poll because the protocol has no push. See `FRICTION.md` #2.
    func refresh(_ botID: String) async {
        guard let space = connection.spaceID, let run = runs[botID] else { return }
        do {
            // State only: the transcript comes from the structured events.
            let status = try await client.status(space: space, runID: run, tail: 1)
            var p = BotPresence(runID: run, status: status)
            // The harness echoes the prompt back as `summary`, marker and all.
            // The marker is this app's private join key and must never be shown.
            p.summary = Self.strippingMarker(p.summary)
            presences[botID] = p
            // One fold per run: a Bot joined to another run starts over.
            let transcript = transcriptRuns[botID] == run
                ? transcripts[botID] ?? CuaSDK.AgentTranscript() : CuaSDK.AgentTranscript()
            transcripts[botID] = transcript
            transcriptRuns[botID] = run
            let before = transcript.revision()
            // New events since the last poll, a page at a time. Absorbing is
            // idempotent, so a re-read page cannot duplicate rows.
            for _ in 0..<Self.eventPagesPerPoll {
                let from = transcript.cursor()
                let page = try await client.events(space: space, runID: run, cursor: from)
                _ = try transcript.absorbJson(json: page)
                if transcript.cursor() == from { break }
            }
            let items = transcript.items()
            if items.contains(where: { $0.kind == "message" }) {
                firstOutputArrived(botID)
            }
            // Two ways a send stops pending, both read off real state:
            //
            // * the reply has **begun**: the run wrote something (a message or
            //   activity) for the turn the message became, so an indicator
            //   next to it would be a lie;
            // * the turn is **over**: the harness says the run is no longer
            //   working, so nothing further is coming. `awaitingInput` counts:
            //   the Bot has stopped and is waiting on the user.
            if let turn = pending[botID] {
                if items.contains(where: { $0.kind != "user" && $0.turn >= turn }) { clearPending(botID) }
                else if p.state != .running { clearPending(botID) }
            }
            if var b = identities[botID] {
                b.preview = transcript.preview() ?? Self.strippingMarker(status.summary)
                b.timestamp = Self.timestamp(Date())
                identities[botID] = b
            }
            if transcript.revision() != before || threads[botID] == nil { rebuildThread(botID) }
            rebuildBots()
        } catch {
            // A failed probe must not be laundered into a state. Say unknown.
            var p = presence(for: botID)
            p.state = .unknown
            p.reason = "status probe failed: \(error)"
            p.acceptsMessage = false
            presences[botID] = p
            // The probe is the only thing that could take the indicator down,
            // and it just failed. Leaving it up would spin forever on a Space
            // that went away; the honest reading of "unknown" is not "still
            // working".
            clearPending(botID)
        }
    }

    /// Event pages read per poll (500 events each) before the rest waits.
    static let eventPagesPerPoll = 20

    func refreshAll() async {
        for id in order where runs[id] != nil { await refresh(id) }
    }

    /// The Bot whose thread is currently on screen, if any.
    ///
    /// Set by `AppModel.open`. The poll loop uses it to decide what deserves a
    /// round trip *this* tick — see `startPolling`.
    private(set) var focusedBotID: String?

    func focus(_ botID: String?) { focusedBotID = botID }

    /// Round-robin cursor over `order`, so every hired Bot is still polled.
    private var rotation = 0

    /// Start the poll loop every consumer of `agent_status` has to write by
    /// hand. One loop for the whole roster, not one per Bot.
    ///
    /// **What this used to do, and why it made the app unusable.** Each tick
    /// called `refreshRoster()` and then `refreshAll()` — one `agent_status`
    /// per hired Bot, serially. On the live Space that is thirty-odd SSH round
    /// trips per tick, and because `BotStore` is `@MainActor` the loop returned
    /// to the main actor between every one of them to parse a 400-line tail
    /// (`rebuildThread`) and republish the whole roster (`rebuildBots`). Thirty
    /// parses and thirty full SwiftUI invalidations per tick, continuously, on
    /// the thread that also has to handle clicks — which is what "unable to
    /// click anything in the top bar without it freezing" was.
    ///
    /// `FRICTION.md` §25 already named the shape of the fix: the *roster* needs
    /// every Bot, but `agent_list` returns state, summary and accepts_message
    /// for every run in **one** call. Only the transcript needs `agent_status`,
    /// and only one transcript is ever on screen. So a tick is now:
    ///
    /// * `agent_list` — one round trip, the whole roster;
    /// * `agent_status` for the focused Bot, if there is one;
    /// * `agent_status` for one other hired Bot, round-robin.
    ///
    /// Two or three round trips instead of thirty-one, and no Bot loses
    /// coverage — the rotation still reaches every one of them, which is what
    /// keeps `testPollLoopRefreshesTheRosterFromOneTask` honest. See §51.
    func startPolling(every interval: Duration = .seconds(2)) {
        pollTask?.cancel()
        pollTask = Task { [weak self] in
            while !Task.isCancelled {
                guard let self else { return }
                await self.pollOnce()
                try? await Task.sleep(for: interval)
            }
        }
    }

    /// One tick of the poll loop. Separated out so a test can drive it.
    func pollOnce() async {
        await refreshRoster()
        let focused = focusedBotID
        if let focused, runs[focused] != nil { await refresh(focused) }
        if let next = nextInRotation(skipping: focused) { await refresh(next) }
    }

    private func nextInRotation(skipping: String?) -> String? {
        let hired = order.filter { runs[$0] != nil && $0 != skipping }
        guard !hired.isEmpty else { return nil }
        rotation = (rotation + 1) % hired.count
        return hired[rotation]
    }

    func stopPolling() { pollTask?.cancel(); pollTask = nil; focusedBotID = nil }

    /// The live window list behind the Agent Computer screens.
    @discardableResult
    func refreshWindows() async -> [SpaceWindow] {
        guard let space = connection.spaceID else { return [] }
        do { spaceWindows = try await client.windows(space: space) }
        catch { post(.failure, nil, "list_space_windows failed: \(error)") }
        return spaceWindows
    }

    // MARK: - Transcript

    /// Rebuild one Bot's thread from what the Space actually reported.
    ///
    /// The thread is derived, not accumulated: every rebuild starts from the
    /// turn list plus the SDK's transcript items, so a re-poll cannot
    /// duplicate rows. The items' `user` entries (the prompt as the agent got
    /// it) are skipped: the user's own words come from the turn list, so the
    /// prompt is never shown twice. Each local turn goes in front of the
    /// first output of the run turn it became; output before any of them
    /// (the greeting a Bot opens with, install progress) comes first.
    func rebuildThread(_ botID: String) {
        let list = turns[botID] ?? []
        let active = presence(for: botID).state == .running
        let items = (transcripts[botID]?.items() ?? []).filter { $0.kind != "user" }
        var messages: [Message] = []
        /// The run turn of each bot message, for reactions.
        var messageTurns: [UInt32?] = []

        if let first = list.first {
            messages.append(Message(sender: .bot, body: .systemEvent(Self.dayLabel(first.startedAt))))
        } else if !items.isEmpty || seeded[botID]?.isEmpty == false {
            messages.append(Message(sender: .bot, body: .systemEvent(Self.dayLabel(Date()))))
        }
        messages.append(contentsOf: seeded[botID] ?? [])
        messageTurns = Array(repeating: nil, count: messages.count)

        var next = 0
        func emitTurns(through last: Int) {
            while next <= last && next < list.count {
                let turn = list[next]
                next += 1
                if let text = turn.userText {
                    messages.append(Message(sender: .user, body: .prose(text)))
                    messageTurns.append(nil)
                }
                for a in turn.attachments {
                    messages.append(Message(sender: .user, body: .linkFile(
                        LinkFile(title: a.name, subtitle: Self.byteLabel(a.byteCount), kind: .file))))
                    messageTurns.append(nil)
                }
                if let reason = turn.refusalReason {
                    // Refusal is shown, never swallowed.
                    messages.append(Message(sender: .bot, body: .systemEvent(
                        Message.refusalPrefix + reason)))
                    messageTurns.append(nil)
                }
            }
        }
        for item in items {
            if let last = list.lastIndex(where: { ($0.runTurn ?? .max) <= item.turn }) {
                emitTurns(through: last)
            }
            if item.kind == "activity" {
                messages.append(Message(sender: .bot, body: .activity(
                    ActivityGroup(summary: item.text, steps: item.steps))))
                messageTurns.append(item.turn)
            } else {
                for body in AgentOutputParser.bodies(from: item.text, active: active) {
                    messages.append(Message(sender: .bot, body: body))
                    messageTurns.append(item.turn)
                }
            }
        }
        emitTurns(through: list.count - 1)
        // A reaction sits under the last thing the Bot said in that turn.
        for turn in list {
            guard let reaction = turn.reaction, let r = turn.runTurn,
                  let i = messages.indices.last(where: { k in
                      messageTurns[k] == r && messages[k].sender == .bot && !Self.isActivity(messages[k])
                  }) else { continue }
            messages[i].reaction = reaction
        }
        threads[botID] = Thread(botID: botID, messages: applyChoiceState(messages, in: botID))
    }

    static func isActivity(_ m: Message) -> Bool {
        if case .activity = m.body { return true }
        return false
    }

    /// Re-apply the user's dealings with choice cards to a freshly rebuilt
    /// thread. Dismissed cards are dropped; answered cards carry their answer.
    private func applyChoiceState(_ messages: [Message], in botID: String) -> [Message] {
        let answers = choiceAnswers[botID] ?? []
        guard !answers.isEmpty || dismissedChoices.contains(botID) else { return messages }
        var out: [Message] = []
        var seen = 0
        for m in messages {
            guard case .choices(var card) = m.body else { out.append(m); continue }
            if dismissedChoices.contains(botID) { continue }
            if seen < answers.count { card.resolved = answers[seen] }
            seen += 1
            out.append(Message(sender: m.sender, body: .choices(card), reaction: m.reaction))
        }
        return out
    }

    // MARK: - Notices

    private func post(_ kind: Notice.Kind, _ botID: String?, _ text: String) {
        notices.append(Notice(kind: kind, botID: botID, text: text))
    }

    func dismiss(_ notice: Notice) {
        notices.removeAll { $0.id == notice.id }
    }

    func clearNotices() { notices.removeAll() }

    var latestNotice: Notice? { notices.last }

    // MARK: - Identity join

    enum StoreError: Error, CustomStringConvertible {
        case notAttached
        case unknownBot(String)
        var description: String {
            switch self {
            case .notAttached: return "no Space attached: call connect() first"
            case .unknownBot(let id): return "no such Bot: \(id)"
            }
        }
    }

    /// The marker written into a run's prompt so a later `agent_list` can be
    /// joined back to the Bot that started it. A run has no metadata field, so
    /// the prompt is the only carrier. `FRICTION.md` #12.
    static func marker(for botID: String) -> String { "[openkoalabots:\(botID)]" }

    static func botID(fromSummary summary: String) -> String? {
        guard let open = summary.range(of: "[openkoalabots:"),
              let close = summary.range(of: "]", range: open.upperBound..<summary.endIndex)
        else { return nil }
        let id = String(summary[open.upperBound..<close.lowerBound])
        return id.isEmpty ? nil : id
    }

    static func strippingMarker(_ summary: String) -> String {
        guard let open = summary.range(of: "[openkoalabots:"),
              let close = summary.range(of: "]", range: open.upperBound..<summary.endIndex)
        else { return summary }
        return (summary.replacingCharacters(in: open.lowerBound..<close.upperBound, with: ""))
            .trimmingCharacters(in: .whitespaces)
    }

    private func adoptedID(for row: AgentRunSummary) -> String { "run:\(row.id)" }

    private func name(_ botID: String) -> String { identities[botID]?.name ?? botID }

    /// A run this app did not start still gets a roster row, drawn from the
    /// same shape/colour vocabulary so it does not look like an error.
    private static func improvisedBot(id: String, row: AgentRunSummary, index: Int) -> Bot {
        let shapes: [BlobShape] = [.circle, .teardrop, .cloud, .hexagon, .egg, .lozenge, .squircle]
        let colors: [UInt32] = [0x8B5CF6, 0x2F80F0, 0x18BE4B, 0x11B5A0, 0xF97316, 0xF4234B, 0x5B5BE8]
        return Bot(id: id,
                   name: row.agent.isEmpty ? "Bot" : row.agent,
                   shape: shapes[index % shapes.count],
                   colorHex: colors[index % colors.count],
                   preview: strippingMarker(row.summary),
                   timestamp: row.createdAt.map { timestamp(Date(timeIntervalSince1970: $0)) } ?? "",
                   screenIndex: index)
    }

    // MARK: - Formatting

    static func timestamp(_ date: Date, now: Date = Date(),
                          calendar: Calendar = .current) -> String {
        if calendar.isDate(date, inSameDayAs: now) {
            let f = DateFormatter()
            f.dateFormat = "h:mm a"
            return f.string(from: date)
        }
        if let y = calendar.date(byAdding: .day, value: -1, to: now),
           calendar.isDate(date, inSameDayAs: y) { return "Yesterday" }
        let f = DateFormatter()
        f.dateFormat = calendar.dateComponents([.day], from: date, to: now).day ?? 0 < 7
            ? "EEEE" : "d MMM"
        return f.string(from: date)
    }

    static func dayLabel(_ date: Date, now: Date = Date(), calendar: Calendar = .current) -> String {
        let f = DateFormatter()
        f.dateFormat = "h:mm a"
        if calendar.isDate(date, inSameDayAs: now) { return "Today \(f.string(from: date))" }
        let d = DateFormatter()
        d.dateFormat = "d MMM"
        return "\(d.string(from: date)) \(f.string(from: date))"
    }

    static func byteLabel(_ bytes: Int) -> String {
        if bytes <= 0 { return "file" }
        if bytes < 1024 { return "\(bytes) B" }
        if bytes < 1024 * 1024 { return String(format: "%.0f KB", Double(bytes) / 1024) }
        return String(format: "%.1f MB", Double(bytes) / (1024 * 1024))
    }
}
