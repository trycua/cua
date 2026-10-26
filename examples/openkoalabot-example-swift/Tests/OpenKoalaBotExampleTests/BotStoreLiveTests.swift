// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation
import Testing
@testable import OpenKoalaBotExample

/// `BotStore` driven against a **live** Space, through the real MCP server.
/// Nothing here is scripted: every assertion is about what the Space returned.
///
/// Run it the same way as the client suite:
///
/// ```sh
/// OPENKOALABOTS_TEST_SPACE=local:cua-space-e3c1b54907 swift test
/// ```
///
/// Unset, it skips rather than creating a sandbox. It obeys the same
/// promises as `SpacesE2ETests`: it never creates, deletes or re-creates a
/// Space and never displays one on the operator's Mac. Every run it starts is
/// stopped and every file it uploads is removed in `tearDown`, pass or fail.
@MainActor
@Suite(.liveSpace, .serialized) final class BotStoreLiveTests {

    private var client: SDKSpacesClient!
    private var store: BotStore!
    private var space: String!
    /// Drained by the `.liveSpace` trait after each test.
    private var cleanup: LiveCleanup { LiveCleanup.current }

    init() throws {
        let target = try LiveSpace.require("the live store suite")
        client = target.client
        space = target.space
        store = BotStore(client: client, identities: Fixtures.bots)
        // XCTest's tearDown stopped polling; the first cleanup registered is
        // the last one drained.
        cleanup.append { [self] in self.store?.stopPolling() }
    }

    @discardableResult
    private func bash(_ command: String) async throws -> String {
        let out = try await client.raw("space_bash", ["space": space!, "command": command])
        return out as? String ?? String(describing: out)
    }

    /// Hire a Bot through the store and register the full teardown the MCP
    /// does not do for us (FRICTION.md #8): stop the run, delete its state
    /// directory and LaunchAgent, close the Terminal window it opened.
    @discardableResult
    private func hire(_ botID: String, _ prompt: String) async throws -> String {
        // Registered *before* anything can throw or assert, and drained in
        // `tearDown`, so the run is cleaned up on the failure path too. The
        // end-of-bundle sweeper in `SpaceHygiene.swift` is the backstop for
        // anything that gets past even this.
        let run = try await store.hire(botID, prompt: prompt)
        cleanup.append { [self] in
            guard let client = self.client, let space = self.space else { return }
            SpaceHygiene.remove(run: run, client: client, space: space)
        }
        return run
    }

    /// Poll the store the way the app does, until the transcript says what we
    /// are waiting for. Returns whether it arrived.
    @discardableResult
    private func pollStore(_ botID: String, seconds: Int = 150,
                           until done: @MainActor () -> Bool) async -> Bool {
        for _ in 0..<seconds {
            await store.refresh(botID)
            if done() { return true }
            try? await Task.sleep(nanoseconds: 1_000_000_000)
        }
        return false
    }

    private func proseTexts(_ botID: String) -> [String] {
        store.thread(for: botID).messages.compactMap { m in
            if case .prose(let t) = m.body, m.sender == .bot { return t }
            return nil
        }
    }

    private func systemTexts(_ botID: String) -> [String] {
        store.thread(for: botID).messages.compactMap { m in
            if case .systemEvent(let t) = m.body { return t }
            return nil
        }
    }

    private func spaceIDs() async throws -> Set<String> {
        let rows = try await client.raw("list_spaces", [:]) as? [[String: Any]] ?? []
        return Set(rows.compactMap { $0["id"] as? String })
    }

    // MARK: - Attach

    /// `connect()` attaches to the one persistent Space and costs nothing.
    @Test func testConnectAttachesToTheWarmSpaceAndCreatesNothing() async throws {
        let before = try await spaceIDs()
        await store.connect()
        XCTAssertEqual(store.spaceID, space, "store attached to \(store.spaceID ?? "nothing")")
        let afterIDs = try await spaceIDs()
        XCTAssertEqual(afterIDs, before,
                       "connect() changed the Space inventory; it must only attach")
        guard case .attached = store.connection else {
            return XCTFail("connection is \(store.connection)")
        }
    }

    // MARK: - The whole live thread

    /// The headline test: hire a Bot in the shared Space, watch its real output
    /// become a transcript, steer it with a follow-up, see the roster carry its
    /// live state, and attach a file — all through the store.
    @Test func testLiveThreadCarriesRealAgentOutputRosterStateAndAttachments() async throws {
        await store.connect()
        let before = try await spaceIDs()

        let run = try await hire("inbox", "Print the single word READY and exit. Do nothing else.")
        XCTAssertTrue(run.hasPrefix("run-"), "agent_start returned \(run)")
        XCTAssertEqual(store.runID(for: "inbox"), run)

        // Real output, reaching the transcript as a bot message.
        let sawReady = await pollStore("inbox") { [self] in
            proseTexts("inbox").contains { $0.contains("READY") }
        }
        XCTAssertTrue(sawReady, "the Bot's own output never reached the transcript. "
                      + "prose was \(proseTexts("inbox"))")

        // The user's prompt is in the thread too — the transcript is the whole
        // conversation, not just the Bot's half.
        XCTAssertTrue(store.thread(for: "inbox").messages.contains { m in
            if case .prose(let t) = m.body, m.sender == .user { return t.contains("READY") }
            return false
        }, "the prompt the user sent is missing from the transcript")
        // The join marker is an implementation detail and must never be shown.
        XCTAssertFalse(store.thread(for: "inbox").messages.contains { m in
            if case .prose(let t) = m.body { return t.contains("[openkoalabots:") }
            return false
        }, "the identity marker leaked into the transcript")

        // Live per-Bot state, from the harness's own vocabulary.
        let p = store.presence(for: "inbox")
        XCTAssertNotEqual(p.state, .unknown, "status never resolved: \(p.reason)")
        XCTAssertFalse(p.reason.isEmpty, "every state should say why")
        XCTAssertFalse(p.summary.isEmpty)
        XCTAssertFalse(p.summary.contains("[openkoalabots:"),
                       "the marker leaked into the user-visible summary")
        XCTAssertFalse(p.label.isEmpty)

        // The roster is live: agent_list joined back to local Bot identity.
        await store.refreshRoster()
        XCTAssertEqual(store.runID(for: "inbox"), run, "agent_list lost the run for this Bot")
        XCTAssertEqual(store.bot("inbox")?.name, "Inbox Manager",
                       "local identity was replaced by the run's agent name")
        XCTAssertTrue(store.bots.contains { $0.id == "inbox" })
        XCTAssertFalse(store.bot("inbox")?.preview.isEmpty ?? true,
                       "the roster preview should carry the Bot's latest utterance")

        // Wait until the harness says a follow-up is legal, then steer it.
        let reachable = await pollStore("inbox", seconds: 90) { [self] in
            store.presence(for: "inbox").acceptsMessage
                || store.presence(for: "inbox").state == .finished
        }
        XCTAssertTrue(reachable, "the run never became reachable: "
                      + "\(store.presence(for: "inbox").state) "
                      + "(\(store.presence(for: "inbox").reason))")

        let outcome = await store.send("Now print the single word SECOND and exit.", to: "inbox")
        XCTAssertTrue(outcome.accepted, "agent_message was refused: \(outcome.reason)")
        let sawSecond = await pollStore("inbox") { [self] in
            proseTexts("inbox").contains { $0.contains("SECOND") }
        }
        XCTAssertTrue(sawSecond, "the follow-up's output never arrived: \(proseTexts("inbox"))")

        // Turn attribution: the second prompt's output belongs to the second
        // turn, so the thread reads user / bot / user / bot.
        let senders = store.thread(for: "inbox").messages.compactMap { m -> String? in
            if case .prose = m.body { return m.sender == .user ? "user" : "bot" }
            return nil
        }
        XCTAssertEqual(senders.prefix(2).map { $0 }, ["user", "bot"],
                       "the transcript does not alternate: \(senders)")
        XCTAssertTrue(senders.dropFirst(2).contains("user"),
                      "the follow-up is missing from the transcript: \(senders)")

        // A reaction rides on the last thing the Bot said.
        store.react("👍", to: "inbox")
        XCTAssertEqual(store.thread(for: "inbox").messages.filter { $0.reaction != nil }.count, 1)

        // Attachment intake through the store, verified inside the Space.
        let tag = UUID().uuidString
        let localPath = NSTemporaryDirectory() + "openkoalabots-store-\(tag).csv"
        try "date,amount\n2026-09-19,\(tag)\n".write(toFile: localPath, atomically: true,
                                                     encoding: .utf8)
        cleanup.append { try? FileManager.default.removeItem(atPath: localPath) }
        let remoteDir = "/tmp/openkoalabots-store-\(tag)"
        try await bash("mkdir -p \(remoteDir)")
        cleanup.append { [self] in _ = try? await self.bash("rm -rf \(remoteDir)") }

        let attachment = await store.attach(localPath, to: "inbox", remoteDirectory: remoteDir)
        XCTAssertNotNil(attachment, "upload failed: \(store.notices.map(\.text))")
        let readBack = try await bash("cat \(remoteDir)/\((localPath as NSString).lastPathComponent)")
        XCTAssertTrue(readBack.contains(tag), "the attachment did not land: \(readBack)")
        XCTAssertTrue(store.thread(for: "inbox").messages.contains { m in
            if case .linkFile(let lf) = m.body, m.sender == .user {
                return lf.title.contains(tag)
            }
            return false
        }, "the attachment was not echoed into the transcript")

        // The live window list the Agent Computer screens open against.
        let windows = await store.refreshWindows()
        if LiveSpace.target?.os.hasPrefix("mac") == true {
            XCTAssertFalse(windows.isEmpty, "a running macOS Space always has windows")
            XCTAssertTrue(windows.allSatisfy { $0.id.hasPrefix("target-") })
        } else {
            // A fresh Linux desktop may have no windows; the ids that are
            // there must be usable.
            XCTAssertTrue(windows.allSatisfy { !$0.id.isEmpty })
        }

        // Stopping is verified by the harness, not assumed.
        let stop = await store.stop("inbox")
        XCTAssertEqual(stop?.stopped, true, "stop did not confirm: \(stop?.reason ?? "nil")")
        XCTAssertEqual(stop?.alive, false)

        // And none of that cost a Space.
        let endIDs = try await spaceIDs()
        XCTAssertEqual(endIDs, before, "the Space inventory changed during the test")
    }

    /// A message sent while the Bot is mid-turn is refused — and the user is
    /// shown the refusal in the transcript, not left to wonder.
    @Test func testMidTurnRefusalIsShownToTheUser() async throws {
        await store.connect()
        _ = try await hire("sales",
                           "Run the shell command `sleep 45` and then print DONE-SLEEP. Do nothing else.")

        let busy = await pollStore("sales", seconds: 60) { [self] in
            store.presence(for: "sales").state == .running
        }
        try XCTSkipUnless(busy, "could not catch the run mid-turn (it was "
                          + "\(store.presence(for: "sales").state)); the refusal path needs a "
                          + "turn in flight")

        // The UI can see the refusal coming before the user types.
        let p = store.presence(for: "sales")
        XCTAssertFalse(p.acceptsMessage, "a running turn should not accept a message")
        XCTAssertNotNil(p.refusalHint, "the chrome has nothing to show the user")

        let outcome = await store.send("interrupt me", to: "sales")
        XCTAssertFalse(outcome.accepted, "a mid-turn message must be refused, not queued")
        XCTAssertFalse(outcome.reason.isEmpty, "a refusal must say why")

        // Surfaced three ways: the return value, a notice, and the transcript.
        XCTAssertEqual(store.notices.last?.kind, .refusal)
        XCTAssertTrue(systemTexts("sales").contains { Message.isRefusal($0) },
                      "the refusal is not in the transcript: \(systemTexts("sales"))")
        XCTAssertTrue(store.thread(for: "sales").messages.contains { m in
            if case .prose(let t) = m.body, m.sender == .user { return t == "interrupt me" }
            return false
        }, "the refused message vanished from the transcript")

        let stop = await store.stop("sales")
        XCTAssertEqual(stop?.stopped, true, "stop did not confirm: \(stop?.reason ?? "nil")")
    }

    /// The store polls the whole roster from one loop, not one loop per Bot.
    /// Proven by starting it against a real Space and watching state arrive.
    @Test func testPollLoopRefreshesTheRosterFromOneTask() async throws {
        await store.connect()
        _ = try await hire("growth", "Print the single word POLLED and exit. Do nothing else.")
        store.startPolling(every: .milliseconds(700))
        defer { store.stopPolling() }

        var arrived = false
        for _ in 0..<120 {
            if proseTexts("growth").contains(where: { $0.contains("POLLED") }) { arrived = true; break }
            try? await Task.sleep(nanoseconds: 1_000_000_000)
        }
        XCTAssertTrue(arrived, "the poll loop never brought the Bot's output in: "
                      + "\(proseTexts("growth"))")
        store.stopPolling()
        _ = await store.stop("growth")
    }
}
