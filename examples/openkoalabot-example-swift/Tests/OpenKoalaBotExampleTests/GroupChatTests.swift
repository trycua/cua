// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpaces
import Foundation
import Testing
@testable import OpenKoalaBotExample

/// Group chats: the 2–6 bound, fan-out, and attribution.
@MainActor
@Suite final class GroupChatTests {

    /// A messenger that answers however the test wants and records the framed
    /// text each Bot actually received.
    final class ScriptedMessenger: GroupMessenger {
        var received: [(bot: String, text: String)] = []
        var accept: (String) -> Bool = { _ in true }
        var refusalReason = "a turn is already in flight"
        var replies: [String: String] = [:]
        var busy: Set<String> = []
        var names: [String: String] = [:]

        func deliver(_ text: String, to botID: String) async -> GroupDelivery {
            received.append((botID, text))
            return accept(botID)
                ? GroupDelivery(botID: botID, accepted: true, reason: "")
                : GroupDelivery(botID: botID, accepted: false, reason: refusalReason)
        }
        func latestReply(from botID: String) async -> String? { replies[botID] }
        func isWorking(_ botID: String) -> Bool { busy.contains(botID) }
        func displayName(_ botID: String) -> String { names[botID] ?? botID }
    }

    private func store(_ messenger: ScriptedMessenger? = nil)
        -> (GroupChatStore, ScriptedMessenger) {
        let m = messenger ?? ScriptedMessenger()
        for b in Fixtures.bots { m.names[b.id] = b.name }
        return (GroupChatStore(messenger: m), m)
    }

    // MARK: - The bound

    @Test func testAGroupOfOneBotIsRefused() {
        let (s, _) = store()
        XCTAssertThrowsError(try s.create(title: "solo", members: ["cos"])) { error in
            XCTAssertEqual(error as? GroupChatError, .tooFewBots(have: 1))
        }
        XCTAssertTrue(s.chats.isEmpty)
    }

    @Test func testAGroupOfSevenBotsIsRefused() {
        let (s, _) = store()
        let seven = Fixtures.bots.prefix(7).map(\.id)
        XCTAssertThrowsError(try s.create(title: "crowd", members: Array(seven))) { error in
            XCTAssertEqual(error as? GroupChatError, .tooManyBots(have: 7))
        }
    }

    @Test func testBothEndsOfTheBoundAreAccepted() throws {
        let (s, _) = store()
        let two = try s.create(title: "pair", members: ["cos", "ea"])
        XCTAssertEqual(two.memberIDs.count, 2)
        let six = try s.create(title: "six", members: Fixtures.bots.prefix(6).map(\.id))
        XCTAssertEqual(six.memberIDs.count, 6)
        XCTAssertTrue(six.isFull)
        XCTAssertTrue(two.isAtFloor)
    }

    @Test func testDuplicateMembersAreCollapsedBeforeTheBoundIsChecked() {
        let (s, _) = store()
        // Three entries, two distinct Bots: that is a group of two, not three.
        let chat = try? s.create(title: "dupes", members: ["cos", "ea", "cos"])
        XCTAssertEqual(chat?.memberIDs, ["cos", "ea"])
        // …and one distinct Bot is still below the floor.
        XCTAssertThrowsError(try s.create(title: "one", members: ["cos", "cos"]))
    }

    /// The ceiling must say what the ceiling is, in the transcript, rather than
    /// failing silently.
    @Test func testAddingToAFullGroupIsRefusedAndTheReasonIsSurfaced() throws {
        let (s, _) = store()
        let chat = try s.create(title: "six", members: Fixtures.bots.prefix(6).map(\.id))
        XCTAssertThrowsError(try s.add("invoice", to: chat.id)) { error in
            XCTAssertEqual(error as? GroupChatError, .full(limit: 6))
        }
        XCTAssertEqual(s.chat(chat.id)?.memberIDs.count, 6)
        XCTAssertEqual(s.lastError, "This group is full: 6 bots is the limit. "
                       + "Remove one to add another.")
        let last = try XCTUnwrap(s.chat(chat.id)?.messages.last)
        XCTAssertEqual(last.speaker, .system)
        XCTAssertTrue(last.undelivered)
        XCTAssertTrue(last.text.contains("6 bots is the limit"))
    }

    @Test func testRemovingBelowTheFloorIsRefusedAndTheReasonIsSurfaced() throws {
        let (s, _) = store()
        let chat = try s.create(title: "pair", members: ["cos", "ea"])
        XCTAssertThrowsError(try s.remove("ea", from: chat.id)) { error in
            XCTAssertEqual(error as? GroupChatError, .atFloor(limit: 2))
        }
        XCTAssertEqual(s.chat(chat.id)?.memberIDs.count, 2)
        XCTAssertTrue(s.lastError?.contains("at least 2 bots") ?? false)
    }

    @Test func testAddAndRemoveWithinTheBoundWorkAndAreAnnounced() throws {
        let (s, _) = store()
        let chat = try s.create(title: "trio", members: ["cos", "ea", "inbox"])
        try s.add("sales", to: chat.id)
        XCTAssertEqual(s.chat(chat.id)?.memberIDs.count, 4)
        XCTAssertEqual(s.chat(chat.id)?.messages.last?.text,
                       "Sales Outbound joined, 4 of 6 bots.")
        try s.remove("inbox", from: chat.id)
        XCTAssertEqual(s.chat(chat.id)?.memberIDs, ["cos", "ea", "sales"])
        XCTAssertEqual(s.chat(chat.id)?.messages.last?.text,
                       "Inbox Manager left, 3 of 6 bots.")
    }

    @Test func testMembershipLabelAndSeatsTrackTheBound() throws {
        let (s, _) = store()
        let chat = try s.create(title: "trio", members: ["cos", "ea", "inbox"])
        XCTAssertEqual(chat.membershipLabel, "3 of 6 bots")
        XCTAssertEqual(chat.remainingSeats, 3)
    }

    @Test func testCanCreateGuardsTheSameBoundAsTheModel() {
        XCTAssertFalse(GroupChatStore.canCreate(with: ["cos"]))
        XCTAssertFalse(GroupChatStore.canCreate(with: ["cos", "cos"]))
        XCTAssertTrue(GroupChatStore.canCreate(with: ["cos", "ea"]))
        XCTAssertTrue(GroupChatStore.canCreate(with: Fixtures.bots.prefix(6).map(\.id)))
        XCTAssertFalse(GroupChatStore.canCreate(with: Fixtures.bots.prefix(7).map(\.id)))
    }

    // MARK: - Fan-out

    @Test func testOneMessageReachesEveryMember() async throws {
        let (s, m) = store()
        let chat = try s.create(title: "launch", members: ["cos", "ea", "inbox"])
        let deliveries = await s.send("Where are we on Thursday?", in: chat.id)
        XCTAssertEqual(deliveries.map(\.botID), ["cos", "ea", "inbox"])
        XCTAssertTrue(deliveries.allSatisfy(\.accepted))
        XCTAssertEqual(m.received.map(\.bot), ["cos", "ea", "inbox"])
    }

    /// Each Bot has to be told what room it is in: the harness gives it no way
    /// to find out, so the group has to be carried in the text.
    @Test func testEachMemberIsToldWhoElseIsInTheRoom() async throws {
        let (s, m) = store()
        let chat = try s.create(title: "launch", members: ["cos", "ea", "inbox"])
        await s.send("status?", in: chat.id)
        let toCos = try XCTUnwrap(m.received.first { $0.bot == "cos" }).text
        XCTAssertTrue(toCos.hasPrefix("[group:launch]"))
        XCTAssertTrue(toCos.contains("EA"))
        XCTAssertTrue(toCos.contains("Inbox Manager"))
        XCTAssertFalse(toCos.contains("Chief of Staff"),
                       "a Bot is not listed as its own peer")
        XCTAssertTrue(toCos.hasSuffix("status?"))
    }

    /// Partial delivery is the normal case, and a Bot that did not hear the
    /// message says so in the transcript instead of just looking quiet.
    @Test func testARefusedMemberIsShownInTheTranscriptNotSwallowed() async throws {
        let (s, m) = store()
        m.accept = { $0 != "ea" }
        let chat = try s.create(title: "launch", members: ["cos", "ea", "inbox"])
        let deliveries = await s.send("status?", in: chat.id)
        XCTAssertEqual(deliveries.filter(\.accepted).map(\.botID), ["cos", "inbox"])

        let messages = try XCTUnwrap(s.chat(chat.id)?.messages)
        let undelivered = messages.filter(\.undelivered)
        XCTAssertEqual(undelivered.count, 1)
        XCTAssertEqual(undelivered[0].speaker, .bot("ea"))
        XCTAssertTrue(undelivered[0].text.contains("a turn is already in flight"))
    }

    // MARK: - Attribution

    @Test func testRepliesAreAttributedToTheBotThatGaveThem() async throws {
        let (s, m) = store()
        let chat = try s.create(title: "launch", members: ["cos", "ea"])
        await s.send("status?", in: chat.id)
        m.replies = ["cos": "Deck is at v5.", "ea": "Thursday is clear."]
        await s.collectReplies(in: chat.id)

        let messages = try XCTUnwrap(s.chat(chat.id)?.messages)
        XCTAssertEqual(messages.first?.speaker, .human)
        let botLines = messages.filter { $0.speaker.botID != nil }
        XCTAssertEqual(botLines.map(\.speaker), [.bot("cos"), .bot("ea")])
        XCTAssertEqual(botLines.map(\.text), ["Deck is at v5.", "Thursday is clear."])
    }

    /// Polling twice must not print a Bot's line twice.
    @Test func testCollectingTwiceDoesNotDuplicateALine() async throws {
        let (s, m) = store()
        let chat = try s.create(title: "launch", members: ["cos", "ea"])
        m.replies = ["cos": "Deck is at v5.", "ea": "Thursday is clear."]
        await s.collectReplies(in: chat.id)
        await s.collectReplies(in: chat.id)
        let lines = try XCTUnwrap(s.chat(chat.id)?.messages).filter { $0.speaker.botID != nil }
        XCTAssertEqual(lines.count, 2)
    }

    @Test func testANewUtteranceFromTheSameBotIsAppended() async throws {
        let (s, m) = store()
        let chat = try s.create(title: "launch", members: ["cos", "ea"])
        m.replies = ["cos": "Working on it."]
        await s.collectReplies(in: chat.id)
        m.replies = ["cos": "Done."]
        await s.collectReplies(in: chat.id)
        let lines = try XCTUnwrap(s.chat(chat.id)?.messages).filter { $0.speaker == .bot("cos") }
        XCTAssertEqual(lines.map(\.text), ["Working on it.", "Done."])
    }

    @Test func testWorkingBotsDriveTheTypingRow() async throws {
        let (s, m) = store()
        let chat = try s.create(title: "launch", members: ["cos", "ea", "inbox"])
        m.busy = ["ea", "inbox"]
        s.refreshWorking(chat.id)
        XCTAssertEqual(s.workingBots(in: chat.id), ["ea", "inbox"])
    }

    @Test func testReactionAttachesToTheLastLine() async throws {
        let (s, m) = store()
        let chat = try s.create(title: "launch", members: ["cos", "ea"])
        m.replies = ["cos": "Deck is at v5."]
        await s.collectReplies(in: chat.id)
        s.react("👍", in: chat.id)
        XCTAssertEqual(s.chat(chat.id)?.messages.last?.reaction, "👍")
    }

    // MARK: - The live messenger, without a Space

    @Test func testTheLiveMessengerHiresUnhiredMembersSoEveryMemberHasARealThread() async {
        let bots = BotStore(client: DemoSpacesClient(), identities: Fixtures.bots)
        await bots.connect()
        let messenger = BotStoreGroupMessenger(store: bots)
        let groups = GroupChatStore(messenger: messenger)
        guard let chat = try? groups.create(title: "launch", members: ["cos", "ea"]) else {
            return XCTFail("group creation failed")
        }
        let deliveries = await groups.send("status?", in: chat.id)
        XCTAssertTrue(deliveries.allSatisfy(\.accepted))
        XCTAssertEqual(bots.runID(for: "cos"), "run-cos")
        XCTAssertEqual(bots.runID(for: "ea"), "run-ea")
    }
}
