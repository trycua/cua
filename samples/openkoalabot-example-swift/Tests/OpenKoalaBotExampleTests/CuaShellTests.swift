// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpaces
import Foundation
import Testing
@testable import OpenKoalaBotExample

/// The desktop shell's affordances.
///
/// These pin the things a screenshot cannot: that the affordance swaps are two
/// controls and not one, that the roster has no concept of an unstarted Bot,
/// that a bot's colour comes from its id, and that no canned model output is
/// shipped as a literal.
@MainActor
@Suite final class CuaShellTests {

    // MARK: The concept is gone

    /// The defect, stated as an assertion: no user-visible string anywhere in
    /// the shell's vocabulary talks about hiring.
    @Test func testNothingInTheShellsVocabularyMentionsHiring() {
        var strings: [String] = [
            EmptyStateCopy.noBots, EmptyStateCopy.noChats, EmptyStateCopy.newAccount,
            EmptyStateCopy.createFirst, EmptyStateCopy.creating,
            EmptyStateCopy.hiddenSection, EmptyStateCopy.showHidden,
            EmptyStateCopy.hiddenExplainer, EmptyStateCopy.teamBotsExplainer,
            EmptyStateCopy.unreachableTitle, EmptyStateCopy.unreachableBody,
            EmptyStateCopy.noMessagesYet,
            SidebarStrings.search,
            ComposeState.toLabel, ComposeState.placeholder,
            ComposeState.composerPlaceholder, ComposeState.createNewBot,
            ComposeState.createGroupChat,
            DetailsStrings.routines, SettingsStrings.title,
            SettingsStrings.notificationsDetail, SettingsStrings.descriptionDefault,
            BotStore.newBotName,
        ]
        strings += DetailsPane.closed.headerControls(botName: "Chief of Staff").map(\.tooltip)
        strings += DetailsPane.details.headerControls(botName: "Chief of Staff").map(\.tooltip)
        strings += DetailsPane.settings.headerControls(botName: "Chief of Staff").map(\.tooltip)
        strings += DetailsPane.details.paneControls().map(\.tooltip)

        for s in strings {
            XCTAssertFalse(s.lowercased().contains("hire"), "still says \"hire\": \(s)")
            XCTAssertFalse(s.lowercased().contains("hired"), "still says \"hired\": \(s)")
        }
    }

    // MARK: The two affordance swaps

    /// The monitor control **unmounts** when the pane opens; the pane's own
    /// close button takes its place. Two controls, not one changing its icon.
    @Test func testMonitorUnmountsAndThePanesCloseButtonTakesOver() {
        var pane = DetailsPane.closed
        var header = pane.headerControls(botName: "Chief of Staff")
        XCTAssertEqual(header.map(\.kind), [.monitor])
        XCTAssertEqual(header.last?.tooltip, "Chief of Staff\u{2019}s Computer")
        XCTAssertTrue(pane.paneControls().isEmpty, "a closed pane has no controls")

        pane = pane.tapping(.monitor)
        XCTAssertEqual(pane, .details)
        header = pane.headerControls(botName: "Chief of Staff")
        XCTAssertFalse(header.contains { $0.kind == .monitor },
                       "the monitor control is still mounted with the pane open")
        XCTAssertEqual(header.map(\.kind), [.gear],
                       "the gear is the only control the details pane mounts")

        let paneControls = pane.paneControls()
        XCTAssertEqual(paneControls.map(\.kind), [.collapse])
        XCTAssertEqual(paneControls.first?.tooltip, "Close details")
        XCTAssertEqual(paneControls[0].kind.self, DetailsPane.Control.Kind.collapse)

        XCTAssertEqual(pane.tapping(.collapse), .closed)
    }

    /// The gear becomes a back `‹`, highlighted, and the close button stays.
    @Test func testGearBecomesAHighlightedBackControlInSettings() {
        let pane = DetailsPane.details.tapping(.gear)
        XCTAssertEqual(pane, .settings)
        XCTAssertEqual(pane.title, "Settings")

        let header = pane.headerControls(botName: "Chief of Staff")
        XCTAssertFalse(header.contains { $0.kind == .gear }, "the gear survived into Settings")
        XCTAssertEqual(header.map(\.kind), [.back])
        XCTAssertTrue(header.last!.isHighlighted, "the back control is drawn highlighted")

        XCTAssertEqual(pane.paneControls().map(\.kind), [.collapse],
                       "the close button stays at the far right in Settings")
        XCTAssertEqual(pane.tapping(.back), .details)
        XCTAssertEqual(pane.tapping(.collapse), .closed)
    }

    /// The gear is gated on ownership and on having somewhere to go.
    @Test func testTheGearIsMountedOnlyForAnOwnerWithSomewhereToGo() {
        XCTAssertFalse(DetailsPane.details
            .headerControls(botName: "B", viewerIsOwner: false)
            .contains { $0.kind == .gear })
        XCTAssertFalse(DetailsPane.details
            .headerControls(botName: "B", canOpenSettings: false)
            .contains { $0.kind == .gear })
    }

    /// The monitor control's own label swaps while the Bot is driving.
    @Test func testTheMonitorLabelSaysWhenTheComputerIsInUse() {
        let idle = DetailsPane.closed.headerControls(botName: "Chief of Staff")
        let busy = DetailsPane.closed.headerControls(botName: "Chief of Staff", computerInUse: true)
        XCTAssertEqual(idle.last?.tooltip, "Chief of Staff\u{2019}s Computer")
        XCTAssertEqual(busy.last?.tooltip, "Chief of Staff\u{2019}s Computer, in use")
    }

    /// The pane narrows the transcript; it never overlays it.
    @Test func testThePaneTakesItsWidthOutOfTheTranscriptRatherThanOverIt() {
        let w: CGFloat = 1280
        let closed = DetailsPane.closed.transcriptWidth(in: w)
        let open = DetailsPane.details.transcriptWidth(in: w)
        XCTAssertLessThan(open, closed, "the transcript did not narrow when the pane opened")
        XCTAssertGreaterThan(DetailsPane.details.paneWidth(in: w), 0)
        // Sidebar + pane + transcript fits inside the window: nothing overlaps.
        XCTAssertLessThanOrEqual(
            Metrics.sidebarW + DetailsPane.details.paneWidth(in: w) + open, w + 1)
    }

    /// In a window too narrow to hold the pane, the header must not act as
    /// though the pane is open: the monitor control has to stay mounted.
    @Test func testANarrowWindowKeepsTheMonitorControlBecauseThePaneCannotOpen() {
        let narrow: CGFloat = 760
        XCTAssertFalse(Metrics.fitsDetails(windowWidth: narrow))
        XCTAssertEqual(DetailsPane.details.paneWidth(in: narrow), 0)

        // The shell derives an *effective* pane from the width, and that is
        // what the header is a function of.
        let effective: DetailsPane =
            DetailsPane.details.paneWidth(in: narrow) > 0 ? .details : .closed
        let header = effective.headerControls(botName: "Chief of Staff")
        XCTAssertTrue(header.contains { $0.kind == .monitor },
                      "the monitor control vanished and no pane replaced it")
        XCTAssertFalse(header.contains { $0.kind == .gear })
    }

    /// The design values in `Metrics`, pinned.
    @Test func testPaneAndSidebarGeometryIsPinned() {
        XCTAssertEqual(Metrics.sidebarW, 280)
        XCTAssertEqual(Metrics.sidebarMinW, 240)
        XCTAssertEqual(Metrics.sidebarMaxW, 400)
        XCTAssertEqual(Metrics.sidebarRailW, 88)
        XCTAssertEqual(Metrics.detailsW, 320)
        XCTAssertEqual(Metrics.detailsMinW, 280)
        XCTAssertEqual(Metrics.transcriptReservedW, 424)
        // windowWidth - 424 - sidebar, clamped.
        XCTAssertEqual(Metrics.detailsWidth(windowWidth: 1280), 320)
        XCTAssertEqual(Metrics.detailsWidth(windowWidth: 1000), 296)
        XCTAssertFalse(Metrics.fitsDetails(windowWidth: 900),
                       "the pane cannot fit without eating the transcript's reservation")
    }

    // MARK: Design geometry

    @Test func testTheRadiusScaleIsTheDesignScale() {
        // 8/10/14/16/18, not the older 6/8/12/14/16 scale.
        XCTAssertEqual([Corner.base, Corner.lg, Corner.xl, Corner.xxl, Corner.xxxl], [8, 10, 14, 16, 18])
        XCTAssertEqual(Corner.bubble, 18)
    }

    @Test func testABubbleIsNotFullWidth() {
        let column = Metrics.transcriptW               // 690
        let max = Metrics.bubbleMaxWidth(in: column)
        // min(88% = 607.2, 640, 690-82 = 608). The **fraction** wins at 690,
        // by less than a point, which is exactly why this is a `min` of three
        // terms in the source rather than a single hard-coded width.
        XCTAssertEqual(max, 607.2, accuracy: 0.01)
        XCTAssertLessThan(max, column)
        // At a narrow column the 82pt gutter wins instead: 400-82 = 318,
        // against 88% = 352.
        XCTAssertEqual(Metrics.bubbleMaxWidth(in: 400), 318, accuracy: 0.01)
        // At a very wide one the absolute cap does.
        XCTAssertEqual(Metrics.bubbleMaxWidth(in: 1200), 640, accuracy: 0.01)
    }

    // MARK: Deterministic personas

    /// Colour and shape come from the agent id, so the same Bot is the same
    /// colour on every machine and after every restart.
    @Test func testPersonaIsDerivedFromTheAgentIdAndIsStable() {
        let a = Persona.resolve(agentID: "new-bot")
        let b = Persona.resolve(agentID: "new-bot")
        XCTAssertEqual(a.colorHex, b.colorHex)
        XCTAssertEqual(a.shape, b.shape)
        XCTAssertTrue(Persona.colorRotation.contains(a.colorName))

        // Different ids land in different places often enough that the
        // derivation is doing work rather than returning a constant.
        let names = Set((0..<64).map { Persona.colorName(agentID: "bot-\($0)") })
        XCTAssertGreaterThan(names.count, 3, "every id got the same colour: \(names)")
        let shapes = Set((0..<64).map { Persona.shapeName(agentID: "bot-\($0)") })
        XCTAssertGreaterThan(shapes.count, 3, "every id got the same shape: \(shapes)")

        // Position in the roster must not matter: the old palette rotation is
        // exactly what that looked like.
        XCTAssertEqual(Persona.bot(id: "x", name: "First").colorHex,
                       Persona.bot(id: "x", name: "Tenth", screenIndex: 9).colorHex)
    }

    @Test func testPersonaHonoursAnExplicitColourAndShape() {
        XCTAssertEqual(Persona.colorName(agentID: "anything", explicit: "magenta"), "magenta")
        XCTAssertEqual(Persona.shapeName(agentID: "anything", explicit: "cloud"), "cloud")
        // An unknown name falls through to the derivation rather than crashing.
        XCTAssertTrue(Persona.colorRotation
            .contains(Persona.colorName(agentID: "anything", explicit: "chartreuse")))
    }

    @Test func testTheHashIsFnv1aOverUtf16CodeUnits() {
        // FNV-1a offset basis with an empty input.
        XCTAssertEqual(Persona.hash(""), 2166136261)
        // One byte: (basis ^ 0x61) * prime, wrapping.
        let expected = (2166136261 ^ UInt32(UInt8(ascii: "a"))) &* 16777619
        XCTAssertEqual(Persona.hash("a"), expected)
        XCTAssertNotEqual(Persona.hash("ab"), Persona.hash("ba"))
    }

    // MARK: The motion catalogue

    @Test func testThereAreThirtyNineMotionStates() {
        XCTAssertEqual(AvatarMotionState.allCases.count, 39)
        XCTAssertEqual(AvatarMotionState.poweringDown.rawValue, "powering-down")
        // Every one resolves to a behaviour the avatar can actually draw.
        let families = Set(AvatarMotionState.allCases.map(\.family))
        XCTAssertEqual(families, Set(BotMotionState.allCases))
    }

    @Test func testActivityProjectsOntoTheCatalogueMostSpecificFirst() {
        var a = AvatarMotionState.Activity(state: .running)
        XCTAssertEqual(AvatarMotionState.project(a), .working)
        a.hasFreshOutput = true
        XCTAssertEqual(AvatarMotionState.project(a), .writing)
        a.isTransferring = true
        XCTAssertEqual(AvatarMotionState.project(a), .uploading)
        a.isComposing = true
        XCTAssertEqual(AvatarMotionState.project(a), .dictating)
        a.isDropTarget = true
        XCTAssertEqual(AvatarMotionState.project(a), .dragging)

        XCTAssertEqual(AvatarMotionState.project(
            .init(state: .finished, exitCode: 0)), .proud)
        XCTAssertEqual(AvatarMotionState.project(
            .init(state: .finished, exitCode: 2)), .sad)
        XCTAssertEqual(AvatarMotionState.project(
            .init(state: .idle, hasThread: false)), .sleeping)
    }

    // MARK: The choice card is a widget, not a script

    /// Example choice-card strings are **not** app strings: they
    /// are model output. Shipping them as literals would make every Bot ask the
    /// same four questions forever.
    @Test func testNoCannedChoiceCardIsShipped() {
        let card = ChoiceCard(heading: "h", options: ["a", "b"])
        XCTAssertEqual(card.freeTextPlaceholder, "Type your own answer")
        XCTAssertEqual(ChoiceCard.letter(0), "A")
        XCTAssertEqual(ChoiceCard.letter(3), "D")
        XCTAssertEqual(ChoiceCard.letter(25), "Z")
        XCTAssertEqual(ChoiceCard.letter(26), "")
    }

    @Test func testAChoiceCardIsParsedOutOfAgentOutput() throws {
        let block = """
        [[choices: What should I start with?]]
        A) Read the backlog
        B) Draft the release notes
        C) Nothing yet
        [[/choices]]
        """
        let card = try XCTUnwrap(AgentOutputParser.choiceCard(in: block))
        XCTAssertEqual(card.heading, "What should I start with?")
        XCTAssertEqual(card.options,
                       ["Read the backlog", "Draft the release notes", "Nothing yet"])
        XCTAssertNil(card.resolved)

        // The emitted letters are ignored: position decides.
        let scrambled = "[[choices: q]]\nC) one\nA) two\n[[/choices]]"
        XCTAssertEqual(AgentOutputParser.choiceCard(in: scrambled)?.options, ["one", "two"])

        // One option is not a choice, and prose is not a card.
        XCTAssertNil(AgentOutputParser.choiceCard(in: "[[choices: q]]\nA) only\n[[/choices]]"))
        XCTAssertNil(AgentOutputParser.choiceCard(in: "just some output"))
    }

    @Test func testAgentOutputContainingAChoiceBlockBecomesAChoiceMessage() {
        let bodies = AgentOutputParser.bodies(
            from: "[[choices: pick]]\nA) one\nB) two\n[[/choices]]", active: false)
        guard case .choices(let card)? = bodies.first else {
            return XCTFail("a choice block did not become a choice card: \(bodies)")
        }
        XCTAssertEqual(card.options.count, 2)
    }

    // MARK: Compose

    @Test func testTheComposeDropdownAlwaysOffersBothCreateRowsAndFiltersBots() {
        let bots = [Persona.bot(id: "alpha", name: "Alpha"),
                    Persona.bot(id: "beta", name: "Beta")]
        let all = ComposeState.suggestions(query: "", bots: bots)
        XCTAssertEqual(all.fixed, ["Create new Bot", "Create group chat"])
        XCTAssertEqual(all.bots.count, 2)

        let filtered = ComposeState.suggestions(query: "alp", bots: bots)
        XCTAssertEqual(filtered.fixed, ["Create new Bot", "Create group chat"],
                       "the create rows must survive a query that matches nothing")
        XCTAssertEqual(filtered.bots.map(\.id), ["alpha"])
    }

    @Test func testTheComposeHeaderStringsArePinned() {
        XCTAssertEqual(ComposeState.toLabel, "To:")
        XCTAssertEqual(ComposeState.placeholder, "Search or create Bots")
        XCTAssertEqual(ComposeState.composerPlaceholder, "Message Bot")
    }

    @Test func testTheDetailsAndSettingsStringsArePinned() {
        XCTAssertEqual(DetailsStrings.screenCaption("Chief of Staff"), "Chief of Staff\u{2019}s screen")
        XCTAssertEqual(DetailsStrings.routines,
                       "Routines are recurring tasks this Bot runs on a schedule. "
                       + "Ask it in chat to set one up.")
        XCTAssertEqual([SettingsStrings.name, SettingsStrings.description,
                        SettingsStrings.voice, SettingsStrings.speed,
                        SettingsStrings.language, SettingsStrings.notifications],
                       ["Name", "Description", "Voice", "Speed", "Language",
                        "Notifications"])
        XCTAssertEqual(SettingsStrings.voiceDefault, "Not set")
        XCTAssertEqual(SettingsStrings.speedDefault, "1x")
        XCTAssertEqual(SettingsStrings.languageDefault, "Auto-detect")
    }

    /// The account row is an identity, not a menu.
    ///
    /// It used to open a menu of six rows, none of which had an action behind
    /// them. They are gone, and this holds them gone: `Account`
    /// carries only what the sidebar actually draws.
    @Test func testTheAccountRowCarriesAnIdentityAndNoDeadMenu() {
        XCTAssertEqual(Account.initials, "AR")
        XCTAssertEqual(Account.firstName, "Alex")
        XCTAssertEqual(Account.displayName, "Alex Rivera")
    }

    // MARK: Creation

    /// The agent is started **before** a row exists, and a failure leaves no
    /// row behind. This is the ordering that makes an "unstarted Bot" state
    /// unreachable rather than merely unlabelled.
    @Test func testCreatingAConversationStartsTheAgentBeforeTheRowExists() async throws {
        let client = ScriptedSpacesClient()
        let store = BotStore(client: client)
        await store.connect()
        XCTAssertTrue(store.bots.isEmpty)

        let bot = try await store.createConversation(named: "New Bot")
        XCTAssertEqual(store.bots.map(\.id), [bot.id])
        XCTAssertTrue(store.presence(for: bot.id).hasThread,
                      "a row exists with no agent behind it")
        XCTAssertTrue(store.creating.contains(bot.id))
        XCTAssertEqual(bot.preview, "", "a brand-new row must have no subtitle")
        // The opening instruction is not a user turn.
        XCTAssertFalse(store.thread(for: bot.id).messages.contains {
            if case .prose = $0.body, $0.sender == .user { return true }
            return false
        }, "the creation prompt leaked into the transcript as the user's message")
    }

    @Test func testANewConversationGoesToTheTopOfTheList() async throws {
        let client = ScriptedSpacesClient()
        let store = BotStore(client: client, identities: Fixtures.bots)
        await store.connect()
        let first = Fixtures.bots.first!.id
        let created = try await store.createConversation(named: "New Bot")
        XCTAssertEqual(store.bots.first?.id, created.id)
        XCTAssertNotEqual(store.bots.first?.id, first)
    }

    @Test func testAFailedCreationLeavesNoRowBehind() async {
        let client = ScriptedSpacesClient()
        let store = BotStore(client: client)
        await store.connect()
        client.failNextStart = true
        do {
            _ = try await store.createConversation(named: "New Bot")
            XCTFail("a failing start still produced a Bot")
        } catch {
            XCTAssertTrue(store.bots.isEmpty,
                          "a row survived a failed create: \(store.bots.map(\.id))")
        }
    }

    @Test func testTheDefaultNewBotNameIsNewBot() {
        // `New Bot`, not `New chat`: the roster is of Bots.
        XCTAssertEqual(BotStore.newBotName, "New Bot")
    }

    // MARK: Choice resolution survives a poll

    @Test func testAnsweringAChoiceCardSurvivesTheNextRebuild() async throws {
        let client = ScriptedSpacesClient()
        let store = BotStore(client: client)
        await store.connect()
        let bot = try await store.createConversation(named: "New Bot")

        client.statusToReturn = AgentStatus(
            state: .awaitingInput, reason: "asked a question", acceptsMessage: true,
            exitCode: nil, summary: "",
            tail: "[[choices: pick]]\nA) one\nB) two\n[[/choices]]")
        await store.refresh(bot.id)
        XCTAssertFalse(store.creating.contains(bot.id),
                       "the row still says Creating… after the Bot spoke")

        await store.answer("one", to: bot.id)
        await store.refresh(bot.id)
        let resolved = store.thread(for: bot.id).messages.compactMap { m -> ChoiceCard? in
            if case .choices(let c) = m.body { return c }
            return nil
        }
        XCTAssertEqual(resolved.first?.resolved, "one",
                       "the answer was erased by the next poll")

        store.dismissChoices(in: bot.id)
        XCTAssertFalse(store.thread(for: bot.id).messages.contains {
            if case .choices = $0.body { return true }
            return false
        }, "a dismissed card came back")
    }

    // MARK: Hidden Bots

    /// A Bot's name survives a restart, and a saved name cannot conjure a row.
    ///
    /// Found by photographing the running app twice: the second launch showed
    /// the first launch's conversation named `claude-code`, because the Space
    /// stores runs and a run has an agent but no Bot name.
    @Test func testRosterNamesSurviveARestartWithoutResurrectingAnything() async throws {
        let file = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("ogb-roster-\(UUID().uuidString).json")
        defer { try? FileManager.default.removeItem(at: file) }

        let first = BotStore(client: ScriptedSpacesClient(), rosterFile: file)
        await first.connect()
        let bot = try await first.createConversation(named: "Inbox Zero")
        XCTAssertTrue(FileManager.default.fileExists(atPath: file.path))

        // A second launch. `agent_list` reports the same run, marked.
        let client = ScriptedSpacesClient()
        client.rosterToReturn = [
            AgentRunSummary(id: "run-scripted", agent: "claude-code", state: .idle,
                            summary: "\(BotStore.marker(for: bot.id)) opening",
                            acceptsMessage: true, createdAt: nil),
        ]
        let second = BotStore(client: client, rosterFile: file)
        await second.connect()
        XCTAssertEqual(second.bot(bot.id)?.name, "Inbox Zero",
                       "the Bot came back wearing its agent's name")
        XCTAssertEqual(second.bot(bot.id)?.colorHex, bot.colorHex)

        // …and with no run reported, the saved name shows nothing at all.
        let third = BotStore(client: ScriptedSpacesClient(), rosterFile: file)
        await third.connect()
        XCTAssertTrue(third.bots.isEmpty,
                      "a saved name resurrected a conversation that has no run: "
                      + "\(third.bots.map(\.id))")
    }

    @Test func testHiddenBotsAreFilteredOutOfTheMainList() {
        var shared = Persona.bot(id: "team", name: "Team Bot")
        shared.isHiddenFromSidebar = true
        let bots = [Persona.bot(id: "mine", name: "Mine"), shared]
        XCTAssertEqual(bots.filter { !$0.isHiddenFromSidebar }.map(\.id), ["mine"])
        XCTAssertEqual(bots.filter(\.isHiddenFromSidebar).map(\.id), ["team"])
        // Default is visible: a Bot the user makes is not hidden.
        XCTAssertFalse(Persona.bot(id: "x", name: "X").isHiddenFromSidebar)
    }

    // MARK: Typing indicator

    @Test func testTheTypingIndicatorIsAPureFunctionOfTime() {
        // Deterministic, so a render path without a run loop gets one frame
        // rather than nothing (`FRICTION.md` §46).
        XCTAssertEqual(ShellTypingIndicator.offset(index: 0, time: 10),
                       ShellTypingIndicator.offset(index: 0, time: 10))
        let offsets = (0..<3).map { ShellTypingIndicator.offset(index: $0, time: 1.0) }
        XCTAssertGreaterThan(Set(offsets.map { Int($0 * 1000) }).count, 1,
                             "all three dots move together")
        for o in offsets { XCTAssertLessThanOrEqual(o, 0) }
    }
}
