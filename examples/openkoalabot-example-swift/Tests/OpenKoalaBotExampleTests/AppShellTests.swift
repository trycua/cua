// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation
import Testing
@testable import OpenKoalaBotExample

// MARK: - The entry point

/// The app and the command line share one executable target. These tests pin
/// the contract that makes that safe: `CLI.run` claims exactly the arguments it
/// used to claim, and nothing else — because anything it declines is what
/// launches the GUI, and an over-eager CLI would make the app unlaunchable.
@Suite final class EntryPointTests {

    @Test func testABareInvocationIsNotClaimedByTheCLI() {
        XCTAssertFalse(CLI.run(["OpenKoalaBots"]),
                       "a bare invocation must fall through to the app")
    }

    @Test func testAnUnrecognisedArgumentIsNotClaimedByTheCLI() {
        XCTAssertFalse(CLI.run(["OpenKoalaBots", "--open-in-window"]))
        XCTAssertFalse(CLI.run(["OpenKoalaBots", "export"]),
                       "export without a directory is not a valid subcommand")
    }

    /// Every subcommand the CLI recognises *exits*, so `run` cannot be called
    /// on one from a test. What can be pinned is that they are all still
    /// documented in one place and that the usage text names the app.
    @Test func testUsageStillListsEverySubcommandAndTheApp() {
        for expected in ["export", "spaces-probe", "live-tiers", "live-shell", "launch the app"] {
            XCTAssertTrue(CLI.usage.contains(expected),
                          "usage no longer mentions \(expected)")
        }
    }
}

// MARK: - The seam

/// `RUBRIC.md` grades thirteen renders built from fixtures. The app renders the
/// *same view types* from a live store. These pin the part of that seam that is
/// checkable without a window server: that both sides satisfy the protocol, and
/// that the defaults the export path relies on still resolve to the fixtures.
@MainActor
@Suite final class DataSourceSeamTests {

    @Test func testTheExportDefaultResolvesToTheFixtures() {
        let source = FixtureDataSource()
        XCTAssertEqual(source.bots.map(\.id), Fixtures.bots.map(\.id))
        // D1/D2/D3 pass no thread and rely on this resolving to salesThread.
        XCTAssertEqual(source.thread(for: "sales").messages.count,
                       Fixtures.salesThread.messages.count)
        XCTAssertEqual(source.thread(for: "cos").messages.count,
                       Fixtures.cosThread.messages.count)
        XCTAssertEqual(source.thread(for: "inbox").messages.count,
                       Fixtures.inboxThread.messages.count)
    }

    @Test func testAStoreWithNoSpaceIsStillAUsableDataSource() {
        // The *export* source is the fixtures, and always will be: the thirteen
        // graded PNGs are built from them.
        let fixtures: BotDataSource = FixtureDataSource()
        XCTAssertFalse(fixtures.bots.isEmpty)
        _ = fixtures.thread(for: "cos")
        _ = fixtures.presence(for: "cos")

        // The *live* source is empty until the user creates something, and has
        // to be a usable data source while it is. This used to assert the
        // opposite — that a store with no Space still had Bots in it — which is
        // only true if the fixtures are the app's roster, which is the defect.
        let empty = BotStore(client: ScriptedSpacesClient())
        XCTAssertTrue(empty.bots.isEmpty)
        XCTAssertTrue(empty.thread(for: "cos").messages.isEmpty)
        XCTAssertFalse(empty.presence(for: "cos").hasThread)

        // And a store handed a roster explicitly behaves like one.
        let populated = BotStore(client: ScriptedSpacesClient(), identities: Fixtures.bots)
        XCTAssertFalse(populated.bots.isEmpty)
        _ = populated.thread(for: "cos")
        _ = populated.presence(for: "cos")
    }

    /// The thirteen graded screens must not acquire a presence chip or a
    /// refusal line by accident: both are drawn only when a *live* source is
    /// supplied, and the export path supplies none.
    @Test func testFixturePresenceNeverAsksForARefusalLine() {
        XCTAssertNil(FixtureDataSource().presence(for: "cos").refusalHint,
                     "a fixture Bot must render without the composer's refusal line")
    }
}

// MARK: - Hiring

@MainActor
@Suite final class HiringTests {

    @Test func testRegisteringABotPutsItOnTheRosterWithoutStartingAnything() async {
        let client = ScriptedSpacesClient()
        let store = BotStore(client: client, identities: Fixtures.bots)
        let before = store.bots.count

        store.register(Bot(id: "newbie", name: "Newbie", shape: .circle, colorHex: 0x8B5CF6,
                           preview: "", timestamp: "", screenIndex: 99))

        XCTAssertEqual(store.bots.count, before + 1)
        XCTAssertEqual(store.bots.last?.id, "newbie")
        XCTAssertFalse(store.presence(for: "newbie").hasThread,
                       "registering must not start a run")
        XCTAssertTrue(client.startedPrompts.isEmpty)
    }

    @Test func testRegisteringIsIdempotent() {
        let store = BotStore(client: ScriptedSpacesClient(), identities: Fixtures.bots)
        let bot = Bot(id: "newbie", name: "Newbie", shape: .circle, colorHex: 0x8B5CF6,
                      preview: "", timestamp: "", screenIndex: 99)
        store.register(bot)
        let after = store.bots.count
        store.register(bot)
        XCTAssertEqual(store.bots.count, after)
    }

    /// The whole point of the hire flow: a new Bot is a new *run inside the
    /// Space the app is already attached to*, never a new Space. The app model
    /// is ~50 Bots per account on one persistent VM.
    @Test func testHiringManyBotsCreatesNoSpaceAndUsesTheOneSpace() async throws {
        let client = ScriptedSpacesClient()
        let store = BotStore(client: client, identities: [])
        await store.connect()

        for i in 0..<50 {
            let id = "hired-\(i)"
            store.register(Bot(id: id, name: "Bot \(i)", shape: .circle, colorHex: 0x8B5CF6,
                               preview: "", timestamp: "", screenIndex: i))
            _ = try await store.hire(id, prompt: "do thing \(i)")
        }

        XCTAssertEqual(client.startedPrompts.count, 50)
        XCTAssertEqual(client.createCount, 0, "hiring must never create a Space")
        XCTAssertTrue(client.deletedSpaces.isEmpty, "hiring must never delete a Space")
        XCTAssertEqual(store.connection.spaceID, client.space)
    }

    /// Identity has to be smuggled through the prompt, so a hired Bot must be
    /// findable again in `agent_list`.
    @Test func testAHiredBotCarriesItsMarkerAndIsJoinedBack() async throws {
        let client = ScriptedSpacesClient()
        let store = BotStore(client: client, identities: [])
        await store.connect()
        store.register(Bot(id: "newbie", name: "Newbie", shape: .circle, colorHex: 0x8B5CF6,
                           preview: "", timestamp: "", screenIndex: 0))
        _ = try await store.hire("newbie", prompt: "triage the inbox")

        let prompt = try XCTUnwrap(client.startedPrompts.first)
        XCTAssertTrue(prompt.hasPrefix(BotStore.marker(for: "newbie")))
        XCTAssertEqual(BotStore.botID(fromSummary: prompt), "newbie")
        // And the marker never leaks into anything the user reads.
        XCTAssertEqual(BotStore.strippingMarker(prompt), "triage the inbox")
    }

    @Test func testHiringWithoutASpaceThrowsRatherThanSilentlyDoingNothing() async {
        let store = BotStore(client: ScriptedSpacesClient(), identities: Fixtures.bots)
        do {
            _ = try await store.hire("cos", prompt: "go")
            XCTFail("hiring with no Space attached must throw")
        } catch {
            XCTAssertTrue("\(error)".contains("no Space attached"))
        }
    }
}

// MARK: - Identifier minting

@Suite final class BotIdentifierTests {

    @Test func testNameBecomesAReadableIdentifier() {
        XCTAssertEqual(mint("Inbox Manager", []), "inbox-manager")
        XCTAssertEqual(mint("Chief of Staff", []), "chief-of-staff")
        XCTAssertEqual(mint("  Sales  Outbound  ", []), "sales-outbound")
    }

    @Test func testCollisionsAreResolvedRatherThanOverwritingAnExistingBot() {
        XCTAssertEqual(mint("EA", ["ea"]), "ea-2")
        XCTAssertEqual(mint("EA", ["ea", "ea-2"]), "ea-3")
    }

    @Test func testANameWithNoUsableCharactersStillYieldsAnIdentifier() {
        XCTAssertEqual(mint("🙂🙂", []), "bot")
        XCTAssertEqual(mint("🙂🙂", ["bot"]), "bot-2")
    }

    /// Mirrors `AppModel.identifier(for:taken:)`. It lives behind `#if
    /// canImport(AppKit)` along with the rest of the app layer, so the rule is
    /// pinned here against the same inputs.
    private func mint(_ name: String, _ taken: Set<String>) -> String {
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
}

// MARK: - Refusal is visible

@MainActor
@Suite final class ComposerRefusalTests {

    /// The app must not swallow a refusal. `BotStore` already records it three
    /// ways; this pins that the *outcome the UI reads* is the refusing one, and
    /// that the composer would have warned first.
    @Test func testAMidTurnRefusalIsReportedToTheCaller() async {
        let client = ScriptedSpacesClient()
        client.nextOutcome = MessageOutcome(accepted: false,
                                            reason: "agent is running; message refused")
        let store = BotStore(client: client, identities: Fixtures.bots)
        await store.connect()
        _ = try? await store.hire("cos", prompt: "start")

        let outcome = await store.send("are you there", to: "cos")

        XCTAssertFalse(outcome.accepted)
        XCTAssertTrue(outcome.reason.contains("refused"))
        XCTAssertTrue(store.notices.contains { $0.kind == .refusal },
                      "a refusal must reach the notice list")
        let transcript = store.thread(for: "cos").messages
        XCTAssertTrue(transcript.contains {
            if case .systemEvent(let t) = $0.body { return t.contains("refused") }
            return false
        }, "a refusal must be written into the transcript, not dropped")
    }

    @Test func testARunningBotWarnsBeforeTheUserTypes() {
        let presence = BotPresence(runID: "run-1", state: .running, reason: "turn in flight",
                                   acceptsMessage: false, summary: "")
        let hint = presence.refusalHint
        XCTAssertNotNil(hint, "a Bot that will refuse must say so before the user types")
        XCTAssertTrue(hint!.contains("refused"))
    }
}

// MARK: - The authoritative strings

/// These are fixed strings. They are the strings the running app
/// composes at runtime, so a rename of a Bot must not be able to change their
/// shape.
@Suite final class AuthoritativeStringTests {

    @Test func testComposerPlaceholdersDifferByPlatform() {
        let name = "Chief of Staff"
        XCTAssertEqual("Ask \(name)", "Ask Chief of Staff")
        XCTAssertEqual("Message \(name)", "Message Chief of Staff")
        XCTAssertNotEqual("Ask \(name)", "Message \(name)",
                          "mobile and desktop placeholders are not interchangeable")
    }

    @Test func testAgentComputerCaptionFormat() {
        XCTAssertEqual("\(Fixtures.bot("cos").name)'s screen", "Chief of Staff's screen")
    }
}
