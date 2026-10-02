// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation
import Testing
@testable import OpenKoalaBotExample

/// Tests for the live data layer: the output parser, the transcript
/// projection, and `BotStore` itself.
///
/// The parser and projection tests need no Space — they are pure. The store
/// tests come in two flavours: a scripted client that lets the refusal and
/// failure paths be forced deterministically, and a **live** suite that drives
/// the real Space through `SDKSpacesClient` end to end.
@Suite final class AgentOutputParserTests {

    @Test func testANSIIsStrippedFromRealTerminalOutput() {
        let raw = "\u{1B}[1;32mREADY\u{1B}[0m\r\n\u{1B}]0;title\u{07}second"
        XCTAssertEqual(AgentOutputParser.strippingANSI(raw), "READY\nsecond")
    }

    @Test func testBareURLBecomesALinkCard() {
        let bodies = AgentOutputParser.bodies(
            from: "https://docs.google.com/presentation/q3-deck.pptx", active: false)
        guard case .linkFile(let lf)? = bodies.first else {
            return XCTFail("expected a link card, got \(bodies)")
        }
        XCTAssertEqual(lf.title, "q3-deck.pptx")
        XCTAssertEqual(lf.subtitle, "docs.google.com")
        XCTAssertEqual(lf.kind, .slides)
    }

    @Test func testBarePathToADocumentBecomesAFileCard() {
        let bodies = AgentOutputParser.bodies(from: "/tmp/openkoalabots/Q3-actuals.csv", active: false)
        guard case .linkFile(let lf)? = bodies.first else {
            return XCTFail("expected a file card, got \(bodies)")
        }
        XCTAssertEqual(lf.title, "Q3-actuals.csv")
        XCTAssertEqual(lf.kind, .sheet)
    }

    @Test func testTitledBlockBecomesACard() {
        let bodies = AgentOutputParser.bodies(from: """
            New email:
            Hi Dan,
            2 PM works on our side.
            """, active: false)
        guard case .card(let c)? = bodies.first else {
            return XCTFail("expected a card, got \(bodies)")
        }
        XCTAssertEqual(c.title, "New email")
        XCTAssertEqual(c.bodyLines, ["Hi Dan,", "2 PM works on our side."])
    }

    @Test func testNarratedComputerWorkBecomesTheTierOneGlyph() {
        let bodies = AgentOutputParser.bodies(from: "Working in Acme CRM", active: true)
        guard case .computerStatus(let cs)? = bodies.first else {
            return XCTFail("expected a computer status, got \(bodies)")
        }
        XCTAssertEqual(cs.text, "Working in Acme CRM")
        XCTAssertTrue(cs.active)
        // A finished run must not draw a live glyph.
        guard case .computerStatus(let spent)? =
                AgentOutputParser.bodies(from: "Working in Acme CRM", active: false).first
        else { return XCTFail("expected a computer status") }
        XCTAssertFalse(spent.active)
    }

    /// The conservative default matters more than the clever cases: output that
    /// is not confidently something richer must be a plain bubble, not a card
    /// with buttons the Bot never offered.
    @Test func testAmbiguousOutputStaysProse() {
        for text in ["Done", "I looked at 40 accounts and found 6 worth chasing.",
                     "ratio: 3", "see https://example.com for the list"] {
            let bodies = AgentOutputParser.bodies(from: text, active: false)
            guard case .prose? = bodies.first else {
                return XCTFail("\(text.prefix(20)) should be prose, got \(bodies)")
            }
        }
    }

    /// Blank lines inside one utterance are **not** message boundaries.
    ///
    /// They used to be, and that is what turned a markdown reply into six
    /// bubbles: a heading, its paragraph, its list and its code block are
    /// blank-line separated by construction, so splitting on blank lines tore
    /// one answer apart and left each fragment rendering on its own.
    /// Consecutive prose is one utterance.
    @Test func testBlankLinesDoNotSplitOneUtteranceIntoSeveralBubbles() {
        let bodies = AgentOutputParser.bodies(from: "first thing\n\nsecond thing\n\n\nthird",
                                              active: false)
        XCTAssertEqual(bodies.count, 1, "prose was split apart again: \(bodies)")
        guard case .prose(let t)? = bodies.first else {
            return XCTFail("expected one prose body, got \(bodies)")
        }
        XCTAssertEqual(t, "first thing\n\nsecond thing\n\nthird")
    }

    /// A card still breaks the run, because a card genuinely is a separate
    /// thing rather than another paragraph of the same answer.
    @Test func testACardStillBreaksARunOfProse() {
        let bodies = AgentOutputParser.bodies(
            from: "before\n\nNew email:\nHi Dan,\n2 PM works.\n\nafter", active: false)
        XCTAssertEqual(bodies.count, 3, "the card did not break the prose run: \(bodies)")
        guard case .prose("before") = bodies[0] else { return XCTFail("\(bodies)") }
        guard case .card = bodies[1] else { return XCTFail("\(bodies)") }
        guard case .prose("after") = bodies[2] else { return XCTFail("\(bodies)") }
    }

    /// A fenced code block survives a blank line inside it.
    ///
    /// The paragraph splitter runs before anything markdown-aware, so a fence
    /// it does not understand is torn in half and the markers are stranded in
    /// two different bubbles. Most agents emit blank lines inside code.
    @Test func testAFencedCodeBlockIsNotSplitByABlankLineInsideIt() {
        let source = "here:\n\n```swift\nlet a = 1\n\nlet b = 2\n```\n\ndone"
        let parts = AgentOutputParser.paragraphs(in: source)
        XCTAssertTrue(parts.contains { $0.contains("let a = 1") && $0.contains("let b = 2") },
                      "the fence was torn in half: \(parts)")
        // And end to end, the block still parses as one code block.
        let bodies = AgentOutputParser.bodies(from: source, active: false)
        guard case .prose(let t)? = bodies.first else {
            return XCTFail("expected prose, got \(bodies)")
        }
        let code = MarkdownParser.blocks(from: t).compactMap { block -> String? in
            if case .code(_, let text) = block { return text }
            return nil
        }
        XCTAssertEqual(code, ["let a = 1\n\nlet b = 2"])
    }

    /// A `Foo:` lead-in over a markdown list is a **list**, not a card.
    ///
    /// The card rule matches `Title:` followed by body lines, which is the
    /// drafted-email shape. A markdown list with a lead-in has exactly that
    /// shape, and being captured by the card rule swallowed the list into
    /// `bodyLines`, where it rendered as flat text with its `-` markers still
    /// showing. The card rule still fires for prose bodies — which is the half
    /// of this that must not regress.
    @Test func testACardLeadInDoesNotSwallowAMarkdownList() {
        let list = AgentOutputParser.bodies(from: "Then:\n- outer\n- inner", active: false)
        guard case .prose? = list.first else {
            return XCTFail("a markdown list became a card: \(list)")
        }
        // …and a genuine card is untouched.
        let card = AgentOutputParser.bodies(
            from: "New email:\nHi Dan,\n2 PM works.", active: false)
        guard case .card(let c)? = card.first else {
            return XCTFail("the email card stopped forming: \(card)")
        }
        XCTAssertEqual(c.title, "New email")
    }

    /// A nested list keeps its nesting through the tail parser.
    ///
    /// The parser used to trim every line before rebuilding a paragraph, so a
    /// sub-bullet arrived flush left and rendered as a sibling of the bullet it
    /// belonged to. The nesting *is* the meaning of a nested list.
    @Test func testANestedListKeepsItsDepthThroughTheTailParser() {
        let source = "Then:\n- outer\n  - inner\n    - deeper"
        let bodies = AgentOutputParser.bodies(from: source, active: false)
        guard case .prose(let t)? = bodies.first else {
            return XCTFail("expected prose, got \(bodies)")
        }
        let depths = MarkdownParser.blocks(from: t).compactMap { block -> Int? in
            if case .listItem(let depth, _, _) = block { return depth }
            return nil
        }
        XCTAssertEqual(depths, [0, 1, 2], "the list was flattened: \(t.debugDescription)")
    }

    /// Indentation inside a code block survives the tail parser.
    ///
    /// It did not: `body(for:)` trimmed every line before rebuilding the
    /// paragraph, so every nested line came out flush left and the code in the
    /// transcript was code that would not compile. This is the single most
    /// damaging thing the parser could do to an agent whose answer is mostly
    /// code, and it was invisible in a one-line example.
    @Test func testIndentationInsideACodeBlockSurvives() {
        let source = """
            ```swift
            func f() {
                if x {
                    return 1
                }
            }
            ```
            """
        let bodies = AgentOutputParser.bodies(from: source, active: false)
        guard case .prose(let t)? = bodies.first else {
            return XCTFail("expected prose, got \(bodies)")
        }
        guard case .code(let lang, let code)? = MarkdownParser.blocks(from: t).first else {
            return XCTFail("expected a code block from \(t)")
        }
        XCTAssertEqual(lang, "swift")
        XCTAssertEqual(code, "func f() {\n    if x {\n        return 1\n    }\n}")
    }

    @Test func testPreviewIsTheLastUtteranceAndFallsBackToSummary() {
        XCTAssertEqual(
            AgentOutputParser.preview(from: "starting\n\nDone", fallback: "x"), "Done")
        XCTAssertEqual(AgentOutputParser.preview(from: "   ", fallback: "tidy the inbox"),
                       "tidy the inbox")
    }

    @Test func testAcknowledgementsAreRecognised() {
        XCTAssertTrue(AgentOutputParser.isAcknowledgement("Done"))
        XCTAssertTrue(AgentOutputParser.isAcknowledgement("  Sent "))
        XCTAssertFalse(AgentOutputParser.isAcknowledgement("Done with the deck"))
    }
}

// MARK: - A scripted client, for the paths a live Space will not produce on demand

/// A `SpacesClient` whose every answer is set by the test. Used only for the
/// branches the live Space cannot be made to produce reliably — a transport
/// failure, a refusal at an exact moment. The happy paths are all tested live.
final class ScriptedSpacesClient: SpacesClient {
    var capabilities = SpacesCapabilities(spaceLifecycle: true, agents: true, windowList: true,
                                          liveScreenPixels: false, upload: true, download: true,
                                          notes: ["scripted"])
    var space = "scripted:space-1"
    var ensureError: Error?
    var startedRun = AgentRun(runID: "run-scripted", agent: "claude-code",
                              space: "scripted:space-1", capabilities: ["turn_model": "single_shot"],
                              notes: [])
    var nextOutcome = MessageOutcome(accepted: true, reason: "delivered")
    var statusToReturn = AgentStatus(state: .running, reason: "turn in flight",
                                     acceptsMessage: false, exitCode: nil,
                                     summary: "", tail: "")
    var statusError: Error?
    /// The next `message` throws rather than returning an outcome. Lets a test
    /// force the transport-failure path, which is one of the ways a pending
    /// send must stop pending.
    var messageError: Error?
    var rosterToReturn: [AgentRunSummary] = []
    var uploadError: Error?
    /// The next `startBot` throws. Lets a test prove that a failed create
    /// leaves no roster row behind.
    var failNextStart = false
    private(set) var startedPrompts: [String] = []
    private(set) var sentTexts: [String] = []
    private(set) var deletedSpaces: [String] = []
    private(set) var createCount = 0

    struct Boom: Error, CustomStringConvertible { var description: String { "scripted failure" } }

    func listSpaces() async throws -> [SpaceSummary] {
        [SpaceSummary(id: space, provider: "local", os: "macos", phase: "running", ip: "1.2.3.4")]
    }
    func ensureSpace() async throws -> String {
        if let ensureError { throw ensureError }
        return space
    }
    func createSpace(on: SpacePlacement, image: String?, wait: Bool) async throws -> SpaceSummary {
        createCount += 1
        return try await listSpaces()[0]
    }
    func deleteSpace(_ space: String) async throws { deletedSpaces.append(space) }
    func startBot(space: String, bot: Bot, prompt: String) async throws -> AgentRun {
        if failNextStart { failNextStart = false; throw Boom() }
        startedPrompts.append(prompt)
        runTurn = 1
        emittedTail = ""
        eventLog.append(["type": "turn_started", "turn": 1, "prompt": prompt])
        return startedRun
    }
    func message(space: String, runID: String, text: String, force: Bool) async throws -> MessageOutcome {
        sentTexts.append(text)
        if let messageError { throw messageError }
        if nextOutcome.accepted {
            runTurn += 1
            eventLog.append(["type": "turn_started", "turn": runTurn, "prompt": text])
        }
        return nextOutcome
    }

    // MARK: Events (agent_events)

    /// The run's event log as `events.jsonl` lines (`type`, `turn`, ...),
    /// oldest first. Tests append to it directly with `record`, and text a
    /// test puts in `statusToReturn.tail` beyond what was already emitted
    /// arrives as the agent's message in the current turn, the way a real
    /// runner writes a reply after a turn starts.
    var eventLog: [[String: Any]] = []
    private(set) var runTurn: UInt32 = 0
    private var emittedTail = ""
    var eventsError: Error?

    func record(_ event: [String: Any]) { eventLog.append(event) }

    /// A raw ACP update line (`agent_message_chunk`, `tool_call`, ...).
    func update(_ kind: String, turn: UInt32? = nil, _ fields: [String: Any] = [:]) {
        var u = fields
        u["sessionUpdate"] = kind
        eventLog.append(["type": "update", "turn": turn ?? runTurn, "update": u])
    }

    func events(space: String, runID: String, cursor: UInt64) async throws -> String {
        if let eventsError { throw eventsError }
        let tail = statusToReturn.tail
        if tail.count > emittedTail.count, tail.hasPrefix(emittedTail) {
            let fresh = String(tail.dropFirst(emittedTail.count))
            update("agent_message_chunk", ["content": ["type": "text", "text": fresh]])
        }
        emittedTail = tail
        let from = Int(min(cursor, UInt64(eventLog.count)))
        let events: [[String: Any]] = eventLog[from...].enumerated().map { i, e in
            var line = e
            line["seq"] = from + i + 1
            line["ts"] = 0
            return line
        }
        let page: [String: Any] = ["events": events, "cursor": eventLog.count, "caught_up": true]
        return String(decoding: try JSONSerialization.data(withJSONObject: page), as: UTF8.self)
    }
    func status(space: String, runID: String, tail: Int) async throws -> AgentStatus {
        if let statusError { throw statusError }
        return statusToReturn
    }
    func stopBot(space: String, runID: String) async throws -> StopOutcome {
        StopOutcome(stopped: true, alive: false, reason: "scripted")
    }
    func listBots(space: String) async throws -> [AgentRunSummary] { rosterToReturn }
    func windows(space: String) async throws -> [SpaceWindow] {
        [SpaceWindow(id: "target-scripted", app: "Chrome", title: "crm.acme.com")]
    }
    func upload(space: String, localPath: String, remotePath: String) async throws {
        if let uploadError { throw uploadError }
    }
    func download(space: String, remotePath: String, localDirectory: String?) async throws -> String {
        (localDirectory ?? "/tmp") + "/" + (remotePath as NSString).lastPathComponent
    }
    func presentScreen(space: String, window: SpaceWindow, tier: ComputerTier) async throws {}
}

@MainActor
@Suite final class BotStoreUnitTests {

    private func store(_ client: ScriptedSpacesClient) -> BotStore {
        BotStore(client: client, identities: Fixtures.bots)
    }

    @Test func testConnectAttachesToOneSpaceAndCreatesNothing() async {
        let c = ScriptedSpacesClient()
        let s = store(c)
        await s.connect()
        XCTAssertEqual(s.spaceID, c.space)
        XCTAssertEqual(c.createCount, 0, "connect() must never create a second sandbox")
        XCTAssertTrue(c.deletedSpaces.isEmpty, "connect() must never delete a Space")
    }

    /// Every Bot's thread lives in the *same* Space. This is the structural
    /// claim in `SpacesClient.swift`, so it is asserted rather than assumed.
    @Test func testEveryBotRunsInTheOneSharedSpace() async throws {
        let c = ScriptedSpacesClient()
        let s = store(c)
        await s.connect()
        c.statusToReturn = AgentStatus(state: .idle, reason: "waiting", acceptsMessage: true,
                                       exitCode: 0, summary: "x", tail: "")
        for id in ["cos", "ea", "inbox"] {
            _ = try await s.hire(id, prompt: "do the \(id) job")
        }
        XCTAssertEqual(c.startedPrompts.count, 3)
        XCTAssertEqual(c.createCount, 0, "a Bot must not cost a Space")
        XCTAssertEqual(s.spaceID, c.space)
    }

    @Test func testConnectFailureIsReportedNotSwallowed() async {
        let c = ScriptedSpacesClient()
        c.ensureError = ScriptedSpacesClient.Boom()
        let s = store(c)
        await s.connect()
        guard case .failed(let why) = s.connection else {
            return XCTFail("connection should be .failed, is \(s.connection)")
        }
        XCTAssertTrue(why.contains("scripted failure"))
        XCTAssertEqual(s.notices.count, 1)
    }

    /// The headline requirement: a refusal reaches the user. It must appear in
    /// the return value, in `notices`, and in the transcript itself.
    @Test func testRefusalSurfacesInOutcomeNoticesAndTranscript() async throws {
        let c = ScriptedSpacesClient()
        let s = store(c)
        await s.connect()
        c.statusToReturn = AgentStatus(state: .running, reason: "turn in flight",
                                       acceptsMessage: false, exitCode: nil,
                                       summary: "tidy the inbox", tail: "working\n")
        _ = try await s.hire("inbox", prompt: "tidy the inbox")

        c.nextOutcome = MessageOutcome(accepted: false, reason: "run is busy with a turn")
        let outcome = await s.send("actually, stop", to: "inbox")

        XCTAssertFalse(outcome.accepted)
        XCTAssertEqual(outcome.reason, "run is busy with a turn")
        XCTAssertEqual(s.notices.last?.kind, .refusal)
        XCTAssertTrue(s.notices.last?.text.contains("run is busy with a turn") == true)

        let events = s.thread(for: "inbox").messages.compactMap { m -> String? in
            if case .systemEvent(let t) = m.body { return t }
            return nil
        }
        XCTAssertTrue(events.contains { Message.isRefusal($0) && $0.contains("busy") },
                      "the refusal is missing from the transcript: \(events)")
        // And the user's own words are still shown — a refused message must not
        // vanish from the thread.
        let userSaid = s.thread(for: "inbox").messages.contains { m in
            if case .prose(let t) = m.body, m.sender == .user { return t == "actually, stop" }
            return false
        }
        XCTAssertTrue(userSaid, "the refused message disappeared from the transcript")
    }

    @Test func testAcceptedMessageIsNotMarkedRefused() async throws {
        let c = ScriptedSpacesClient()
        let s = store(c)
        await s.connect()
        c.statusToReturn = AgentStatus(state: .idle, reason: "idle", acceptsMessage: true,
                                       exitCode: 0, summary: "s", tail: "first\n")
        _ = try await s.hire("inbox", prompt: "tidy the inbox")
        c.nextOutcome = MessageOutcome(accepted: true, reason: "delivered")
        let outcome = await s.send("and archive the rest", to: "inbox")
        XCTAssertTrue(outcome.accepted)
        XCTAssertFalse(s.notices.contains { $0.kind == .refusal })
        XCTAssertFalse(s.thread(for: "inbox").messages.contains { m in
            if case .systemEvent(let t) = m.body { return Message.isRefusal(t) }
            return false
        })
    }

    /// A status probe that fails must leave the Bot `unknown`, never laundered
    /// into a state that looks like health.
    @Test func testFailedStatusProbeDegradesToUnknown() async throws {
        let c = ScriptedSpacesClient()
        let s = store(c)
        await s.connect()
        c.statusToReturn = AgentStatus(state: .idle, reason: "ok", acceptsMessage: true,
                                       exitCode: 0, summary: "s", tail: "")
        _ = try await s.hire("inbox", prompt: "tidy")
        c.statusError = ScriptedSpacesClient.Boom()
        await s.refresh("inbox")
        XCTAssertEqual(s.presence(for: "inbox").state, .unknown)
        XCTAssertFalse(s.presence(for: "inbox").acceptsMessage)
        XCTAssertTrue(s.presence(for: "inbox").reason.contains("scripted failure"))
    }

    /// `accepts_message` must reach the UI, not just the model.
    @Test func testPresenceExposesAcceptsMessageToTheChrome() {
        let busy = BotPresence(runID: "run-1", state: .running, reason: "turn in flight",
                               acceptsMessage: false, summary: "s")
        XCTAssertEqual(busy.label, "Working")
        XCTAssertNotNil(busy.refusalHint)
        let free = BotPresence(runID: "run-1", state: .awaitingInput, reason: "asked a question",
                               acceptsMessage: true, summary: "s")
        XCTAssertEqual(free.label, "Waiting for you")
        XCTAssertNil(free.refusalHint)
        XCTAssertNil(BotPresence.unstarted.runID)
        // The label says what is *known*, not what has not been done to the
        // Bot. "Not hired" was an employment metaphor for a state the design
        // does not have, and the state it named cannot be reached in the app:
        // a Bot's agent is started before its row exists.
        XCTAssertEqual(BotPresence.unstarted.label, "Unknown")
        XCTAssertFalse(BotPresence.unstarted.hasThread)
        for presence in [BotPresence.unstarted,
                         BotPresence(runID: "r", state: .unknown),
                         BotPresence(runID: "r", state: .running)] {
            XCTAssertFalse(presence.label.lowercased().contains("hire"),
                           "a user-visible label still talks about hiring: \(presence.label)")
            XCTAssertFalse((presence.refusalHint ?? "").lowercased().contains("hire"),
                           "a composer hint still talks about hiring")
        }
    }

    /// The store the app actually constructs starts with **no** roster.
    @Test func testTheDefaultRosterIsEmpty() {
        let s = BotStore(client: ScriptedSpacesClient())
        XCTAssertTrue(s.bots.isEmpty,
                      "the live store still ships fixture Bots: \(s.bots.map(\.id))")
        XCTAssertFalse(s.adoptsForeignRuns)
    }

    /// The transcript is *derived* from the turn list plus the current tail, so
    /// polling repeatedly must not duplicate rows.
    @Test func testRepeatedPollsDoNotDuplicateTranscriptRows() async throws {
        let c = ScriptedSpacesClient()
        let s = store(c)
        await s.connect()
        c.statusToReturn = AgentStatus(state: .running, reason: "running", acceptsMessage: false,
                                       exitCode: nil, summary: "s", tail: "one\n\ntwo\n")
        _ = try await s.hire("inbox", prompt: "go")
        let first = s.thread(for: "inbox").messages.count
        for _ in 0..<5 { await s.refresh("inbox") }
        XCTAssertEqual(s.thread(for: "inbox").messages.count, first,
                       "re-polling duplicated transcript rows")
    }

    /// Output produced after a follow-up must be attributed to that turn, so
    /// the transcript reads user / bot / user / bot rather than dumping all of
    /// the output at the end.
    @Test func testOutputIsAttributedToTheTurnThatCausedIt() async throws {
        let c = ScriptedSpacesClient()
        let s = store(c)
        await s.connect()
        c.statusToReturn = AgentStatus(state: .idle, reason: "idle", acceptsMessage: true,
                                       exitCode: 0, summary: "s", tail: "READY\n")
        _ = try await s.hire("inbox", prompt: "print READY")
        c.statusToReturn.tail = "READY\n\nSECOND\n"
        _ = await s.send("print SECOND", to: "inbox")
        await s.refresh("inbox")

        let shape: [String] = s.thread(for: "inbox").messages.map { m in
            switch m.body {
            case .systemEvent: return "sys"
            case .prose(let t): return "\(m.sender == .user ? "user" : "bot"):\(t)"
            default: return "other"
            }
        }
        XCTAssertEqual(shape, ["sys", "user:print READY", "bot:READY",
                               "user:print SECOND", "bot:SECOND"],
                       "turn attribution is wrong: \(shape)")
    }

    @Test func testReactionAttachesToTheLastBotMessageOfATurn() async throws {
        let c = ScriptedSpacesClient()
        let s = store(c)
        await s.connect()
        c.statusToReturn = AgentStatus(state: .finished, reason: "exited", acceptsMessage: false,
                                       exitCode: 0, summary: "s", tail: "one\n\ntwo\n")
        _ = try await s.hire("cos", prompt: "go")
        s.react("👍", to: "cos")
        let reacted = s.thread(for: "cos").messages.filter { $0.reaction != nil }
        XCTAssertEqual(reacted.count, 1, "the reaction landed on more than one row")
        // `one` and `two` are one utterance now — blank lines inside a reply no
        // longer split it — so the turn's last bot message carries both. What
        // this test is about is unchanged: exactly one row is reacted to, and
        // it is the turn's final bot message.
        if case .prose(let t) = reacted.first!.body {
            XCTAssertEqual(t, "one\n\ntwo")
        } else {
            XCTFail("reaction landed on the wrong row")
        }
        XCTAssertEqual(reacted.first!.sender, .bot)
    }

    @Test func testAttachmentIsEchoedIntoTheTranscript() async throws {
        let c = ScriptedSpacesClient()
        let s = store(c)
        await s.connect()
        c.statusToReturn = AgentStatus(state: .idle, reason: "idle", acceptsMessage: true,
                                       exitCode: 0, summary: "s", tail: "")
        _ = try await s.hire("inbox", prompt: "go")
        let path = NSTemporaryDirectory() + "openkoalabots-unit-\(UUID().uuidString).csv"
        try "a,b\n1,2\n".write(toFile: path, atomically: true, encoding: .utf8)
        defer { try? FileManager.default.removeItem(atPath: path) }

        let a = await s.attach(path, to: "inbox")
        XCTAssertNotNil(a)
        let echoed = s.thread(for: "inbox").messages.contains { m in
            if case .linkFile(let lf) = m.body, m.sender == .user {
                return lf.title == (path as NSString).lastPathComponent
            }
            return false
        }
        XCTAssertTrue(echoed, "the attachment never appeared in the thread")
    }

    @Test func testUploadFailureIsReportedNotSwallowed() async throws {
        let c = ScriptedSpacesClient()
        c.uploadError = ScriptedSpacesClient.Boom()
        let s = store(c)
        await s.connect()
        let a = await s.attach("/tmp/whatever.csv", to: "inbox")
        XCTAssertNil(a)
        XCTAssertEqual(s.notices.last?.kind, .failure)
    }

    /// The roster join: a run started by this app carries a marker in its
    /// prompt (and therefore in its summary), which is how `agent_list` rows
    /// become named Bots again after a relaunch.
    @Test func testRosterJoinsRunsBackToLocalBotIdentity() async throws {
        let c = ScriptedSpacesClient()
        let s = store(c)
        await s.connect()
        c.rosterToReturn = [
            AgentRunSummary(id: "run-aaaa1111", agent: "claude-code", state: .awaitingInput,
                            summary: "\(BotStore.marker(for: "cos")) draft the board deck",
                            acceptsMessage: true, createdAt: Date().timeIntervalSince1970),
        ]
        await s.refreshRoster()
        XCTAssertEqual(s.runID(for: "cos"), "run-aaaa1111")
        XCTAssertEqual(s.presence(for: "cos").state, .awaitingInput)
        XCTAssertTrue(s.presence(for: "cos").acceptsMessage)
        XCTAssertEqual(s.presence(for: "cos").summary, "draft the board deck",
                       "the join marker leaked into the user-visible summary")
        XCTAssertEqual(s.bot("cos")?.name, "Chief of Staff", "local identity was lost")
    }

    /// A run this app did not start is **not** a conversation, and does not
    /// get a row.
    ///
    /// This assertion is the reverse of the one it replaces, and the reversal
    /// is the fix. The old rule — adopt every run in the Space, because a Bot
    /// working invisibly is worse than an unfamiliar row — is defensible in the
    /// abstract and was wrong in practice: the live Space has thirty-odd runs
    /// in it, and the user opened the app to "a bunch of fake threads in my
    /// DMs". The sidebar lists conversations the user had. Adoption is still
    /// available behind a flag, and the flag's other setting is asserted here
    /// too, so switching it back on is a decision rather than a regression.
    @Test func testUnmarkedRunsDoNotBecomeConversationsUnlessAdoptionIsAskedFor() async throws {
        let foreign = [
            AgentRunSummary(id: "run-bbbb2222", agent: "claude-code", state: .running,
                            summary: "something started elsewhere", acceptsMessage: false,
                            createdAt: nil),
        ]

        let quiet = ScriptedSpacesClient()
        let s = BotStore(client: quiet, identities: Fixtures.bots)
        await s.connect()
        quiet.rosterToReturn = foreign
        await s.refreshRoster()
        XCTAssertFalse(s.bots.contains { $0.id == "run:run-bbbb2222" },
                       "a run nobody started here became a conversation: \(s.bots.map(\.id))")

        let adopting = ScriptedSpacesClient()
        let a = BotStore(client: adopting, identities: Fixtures.bots,
                         adoptsForeignRuns: true)
        await a.connect()
        adopting.rosterToReturn = foreign
        await a.refreshRoster()
        XCTAssertTrue(a.bots.contains { $0.id == "run:run-bbbb2222" },
                      "adoption was asked for and did not happen")
        XCTAssertEqual(a.presence(for: "run:run-bbbb2222").state, .running)
    }

    @Test func testMarkerRoundTrips() {
        let summary = "\(BotStore.marker(for: "sales")) chase 40 accounts"
        XCTAssertEqual(BotStore.botID(fromSummary: summary), "sales")
        XCTAssertEqual(BotStore.strippingMarker(summary), "chase 40 accounts")
        XCTAssertNil(BotStore.botID(fromSummary: "no marker here"))
        XCTAssertEqual(BotStore.strippingMarker("no marker here"), "no marker here")
    }

    @Test func testSendingToAnUnhiredBotIsRefusedWithAReason() async {
        let c = ScriptedSpacesClient()
        let s = store(c)
        await s.connect()
        let outcome = await s.send("hello", to: "growth")
        XCTAssertFalse(outcome.accepted)
        XCTAssertFalse(outcome.reason.isEmpty)
        XCTAssertTrue(c.sentTexts.isEmpty, "a message was sent for a Bot with no run")
    }
}

// MARK: - The export path must keep working

@MainActor
@Suite final class FixtureDataSourceTests {

    /// The export renders are regression baselines, so the fixture
    /// source must stay exactly what it was: same Bots, same threads.
    @Test func testFixtureSourceIsUnchangedFixtureData() {
        let source = FixtureDataSource()
        XCTAssertEqual(source.bots.map(\.id), Fixtures.bots.map(\.id))
        XCTAssertEqual(source.thread(for: "inbox").messages.count,
                       Fixtures.inboxThread.messages.count)
        XCTAssertEqual(source.thread(for: "cos").messages.count,
                       Fixtures.cosThread.messages.count)
        XCTAssertEqual(source.thread(for: "sales").messages.count,
                       Fixtures.salesThread.messages.count)
    }

    /// The store and the fixtures are interchangeable to a view: both satisfy
    /// the one protocol the screens read through.
    @Test func testStoreAndFixturesSatisfyTheSameDataSource() {
        let sources: [BotDataSource] = [FixtureDataSource(),
                                        BotStore(client: DemoSpacesClient())]
        for s in sources {
            _ = s.bots
            _ = s.thread(for: "cos")
            _ = s.presence(for: "cos")
        }
    }

    /// The export still has something to render, and it is the desktop.
    ///
    /// This used to assert thirteen screens, ten of them `mobile-*`. The phone
    /// surface is gone, so the list is the three desktop screens, and the
    /// assertion is that `export` is not silently empty rather than that a
    /// pixel count held.
    @Test func testTheExportRendersTheSurvivingDesktopScreens() {
        let names = screens().map(\.name)
        XCTAssertEqual(names, ["desktop-01-dark", "desktop-02-light", "desktop-03-no-panel"],
                       "the export screen list changed: \(names)")
        XCTAssertTrue(names.allSatisfy { $0.hasPrefix("desktop-") },
                      "a non-desktop screen came back into the export: \(names)")
    }
}

// MARK: - Activity is muted, messages are messages

/// The transcript is built from the SDK's classification of the run's events
/// (`AgentTranscript`), not from output text: install progress, tool calls
/// and results and turn ends become one muted activity group per stretch,
/// the agent's words stay prose, and the prompt is never echoed as agent
/// text.
@MainActor
@Suite final class TranscriptActivityTests {

    private func kinds(_ s: BotStore, _ id: String) -> [String] {
        s.thread(for: id).messages.map { m in
            switch m.body {
            case .systemEvent: return "sys"
            case .prose(let t): return "\(m.sender == .user ? "user" : "bot"):\(t)"
            case .activity(let g): return "activity:\(g.summary)"
            default: return "other"
            }
        }
    }

    @Test func testHarnessLinesAreOneMutedGroupAndNeverAMessage() async throws {
        let c = ScriptedSpacesClient()
        let s = BotStore(client: c, identities: Fixtures.bots)
        await s.connect()
        c.statusToReturn = AgentStatus(state: .idle, reason: "idle", acceptsMessage: true,
                                       exitCode: 0, summary: "click it", tail: "")
        c.record(["type": "install", "turn": 0, "id": "node", "phase": "cached", "detail": ""])
        c.record(["type": "install", "turn": 0, "id": "claude-code", "phase": "done", "detail": ""])
        _ = try await s.hire("cos", prompt: "click it")
        c.update("user_message_chunk", ["content": ["type": "text", "text": "click it"]])
        c.update("agent_message_chunk", ["content": ["type": "text", "text": "On it."]])
        c.update("tool_call", ["toolCallId": "t1", "title": "mcp__cua-driver__click", "kind": "other",
                               "status": "pending"])
        c.update("tool_call_update", ["toolCallId": "t1", "status": "completed",
                                      "content": [["type": "content",
                                                   "content": ["type": "text", "text": "{\"ok\":true}"]]]])
        c.update("agent_message_chunk", ["content": ["type": "text", "text": "Clicked."]])
        c.record(["type": "turn_ended", "turn": 1, "stopReason": "end_turn"])
        await s.refresh("cos")

        XCTAssertEqual(kinds(s, "cos"), ["sys", "activity:2 steps", "user:click it", "bot:On it.",
                                         "activity:1 step", "bot:Clicked.", "activity:1 step"])
        let steps = s.thread(for: "cos").messages.compactMap { m -> [String]? in
            if case .activity(let g) = m.body { return g.steps }
            return nil
        }
        XCTAssertEqual(steps, [["Install node: cached", "Install claude-code: done"],
                               ["Tool mcp__cua-driver__click completed: {\"ok\":true}"],
                               ["Turn 1 ended (end_turn)"]])
        let prose = s.thread(for: "cos").messages.compactMap { m -> String? in
            if case .prose(let t) = m.body { return t }
            return nil
        }
        XCTAssertFalse(prose.contains { $0.hasPrefix("[") }, "a harness line became a message: \(prose)")
        XCTAssertEqual(s.bots.first { $0.id == "cos" }?.preview, "Clicked.",
                       "the preview is the last agent message, never a turn marker")
    }

    @Test func testFollowUpActivityGoesUnderItsOwnTurn() async throws {
        let c = ScriptedSpacesClient()
        let s = BotStore(client: c, identities: Fixtures.bots)
        await s.connect()
        c.statusToReturn = AgentStatus(state: .idle, reason: "idle", acceptsMessage: true,
                                       exitCode: 0, summary: "s", tail: "")
        _ = try await s.hire("cos", prompt: "first")
        c.update("agent_message_chunk", ["content": ["type": "text", "text": "one"]])
        c.record(["type": "turn_ended", "turn": 1, "stopReason": "end_turn"])
        await s.refresh("cos")
        _ = await s.send("second", to: "cos")
        c.update("agent_thought_chunk", ["content": ["type": "text", "text": "hmm"]])
        c.update("agent_message_chunk", ["content": ["type": "text", "text": "two"]])
        await s.refresh("cos")
        XCTAssertEqual(kinds(s, "cos"), ["sys", "user:first", "bot:one", "activity:1 step",
                                         "user:second", "activity:1 step", "bot:two"])
        XCTAssertFalse(s.isAwaitingReply("cos"))
    }
}

@MainActor
@Suite final class RunJoinTests {
    /// An earlier launch's run with the same marker must not take over the
    /// Bot's thread (nor mix its events into it).
    @Test func testAnOlderRunWithTheSameMarkerDoesNotTakeOver() async throws {
        let c = ScriptedSpacesClient()
        let s = BotStore(client: c, identities: Fixtures.bots)
        await s.connect()
        c.statusToReturn = AgentStatus(state: .idle, reason: "idle", acceptsMessage: true,
                                       exitCode: 0, summary: "s", tail: "")
        let run = try await s.hire("cos", prompt: "go")
        let marker = BotStore.marker(for: "cos")
        c.rosterToReturn = [
            AgentRunSummary(id: run, agent: "claude-code", state: .idle, summary: "\(marker) go",
                            acceptsMessage: true, createdAt: 2),
            AgentRunSummary(id: "run-old", agent: "claude-code", state: .idle, summary: "\(marker) old",
                            acceptsMessage: true, createdAt: 1),
        ]
        await s.refreshRoster()
        XCTAssertEqual(s.runID(for: "cos"), run)
    }
}
