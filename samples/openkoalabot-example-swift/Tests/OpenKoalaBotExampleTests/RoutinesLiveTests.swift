// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpaces
import Foundation
import Testing
@testable import OpenKoalaBotExample

/// The scheduler, against a **live** Space.
///
/// A timer that logs is not a scheduler. What this suite proves is the only
/// thing that matters about routines: the clock coming round starts a **real
/// agent run inside the shared Space**, that run is visible in `agent_list`,
/// and it is stopped and cleaned up afterwards.
///
/// ```sh
/// OPENKOALABOTS_TEST_SPACE=local:cua-space-e3c1b54907 swift test
/// ```
///
/// Same promises as the rest of the live suites: never creates, deletes or
/// re-creates a Space, never displays one on the operator's Mac, and every
/// run it starts is stopped in teardown whether the test passed or failed.
@MainActor
@Suite(.liveSpace, .serialized) final class RoutinesLiveTests {

    private var client: SDKSpacesClient!
    private var bots: BotStore!
    private var routines: RoutineStore!
    private var space: String!
    private var file: URL!
    /// Drained by the `.liveSpace` trait after each test.
    private var cleanup: LiveCleanup { LiveCleanup.current }

    init() throws {
        let target = try LiveSpace.require("the live routines suite")
        client = target.client
        space = target.space
        bots = BotStore(client: client, identities: Fixtures.bots)
        file = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("openkoalabots-live-routines-\(UUID().uuidString).json")
        routines = RoutineStore(fileURL: file)
        routines.attach(runner: BotStoreRoutineRunner(store: bots))
        // XCTest's tearDown, drained last.
        cleanup.append { [self] in
            self.routines?.stopScheduler()
            if let file = self.file { try? FileManager.default.removeItem(at: file) }
            self.bots?.stopPolling()
        }
    }

    @discardableResult
    private func bash(_ command: String) async throws -> String {
        let out = try await client.raw("space_bash", ["space": space!, "command": command])
        return out as? String ?? String(describing: out)
    }

    /// The full teardown for a run the MCP will not do for us (FRICTION.md #8):
    /// stop the agent, kill its log watcher, delete its run directory and
    /// LaunchAgent, close the Terminal window it opened.
    private func registerCleanup(for run: String) {
        cleanup.append { [self] in
            guard let client = self.client, let space = self.space else { return }
            SpaceHygiene.remove(run: run, client: client, space: space)
        }
    }

    /// Register teardown for **every** run the Space has gained since this test
    /// started, whether or not this test ever learned its id.
    ///
    /// The routines suite is the one that cannot name its runs up front: a
    /// routine fires on a timer inside `RoutineStore`'s scheduler and a group
    /// chat fans out to one run per member, so the ids only exist after the
    /// fact — and the old code registered them *after* an `XCTUnwrap` or a
    /// `guard case … else { return XCTFail() }`, which meant a failing
    /// assertion leaked every run it had just started. Diffing `agent_list`
    /// against a baseline needs no id from the test at all, so there is nothing
    /// left for a failure to skip past.
    private func adoptNewRuns() async {
        guard let client, let space else { return }
        let now = SpaceHygiene.runIDs(client: client, space: space)
        for run in now.subtracting(baselineRuns) where !adopted.contains(run) {
            adopted.insert(run)
            registerCleanup(for: run)
        }
    }

    /// Runs already in the Space when this test began — never ours to remove.
    private var baselineRuns: Set<String> = []
    private var adopted: Set<String> = []

    /// Call at the top of any test that starts runs indirectly.
    private func recordBaseline() {
        guard let client, let space else { return }
        baselineRuns = SpaceHygiene.runIDs(client: client, space: space)
    }

    /// The Space's own view of what is running, straight from `agent_list`.
    private func liveRunIDs() async throws -> Set<String> {
        Set(try await client.listBots(space: space).map(\.id))
    }

    /// **The scheduler proof.**
    ///
    /// A routine that came due one tick ago fires, and the firing is a real
    /// `agent_start` in `local:cua-space-e3c1b54907` — the run id it returns is
    /// present in the Space's own `agent_list`, and stopping it is confirmed by
    /// the Space rather than assumed.
    @Test func testADueRoutineStartsARealAgentRunInTheSpace() async throws {
        await bots.connect()
        recordBaseline()
        XCTAssertEqual(bots.spaceID, space)

        let before = try await liveRunIDs()

        // A one-minute routine created two minutes ago is due *now*, so the
        // tick fires it without the test sleeping through a schedule.
        let createdAt = Date().addingTimeInterval(-120)
        let routine = routines.create(
            botID: "ea", title: "OpenKoalaBots live scheduler proof",
            prompt: "Print the single word SCHEDULED and exit.",
            schedule: .everyMinutes(1), now: createdAt)
        XCTAssertTrue(routine.isDue(at: Date()), "the routine should be due")
        XCTAssertEqual(routines.due(at: Date()).map(\.id), [routine.id])

        let fired = await routines.tick(now: Date())
        // Adopt whatever the tick actually started before asserting anything
        // about it: an assertion that fails here must not also leak a run.
        await adoptNewRuns()
        XCTAssertEqual(fired.count, 1, "the tick should have fired exactly one routine")
        guard case .started(let runID) = fired[0].firing else {
            return XCTFail("routine did not start a run: \(fired[0].firing)")
        }
        XCTAssertFalse(runID.isEmpty)

        // The Space itself agrees a new run exists.
        let after = try await liveRunIDs()
        XCTAssertTrue(after.contains(runID),
                      "run \(runID) is not in the Space's agent_list: \(after)")
        XCTAssertFalse(before.contains(runID), "the run is new, not one that was already there")
        print("SCHEDULER PROOF: routine '\(routine.title)' fired -> run \(runID) "
              + "in \(space!); agent_list grew from \(before.count) to \(after.count)")

        // The routine records what it did, and that record is on disk.
        let saved = try XCTUnwrap(RoutineStore(fileURL: file).routine(routine.id))
        XCTAssertEqual(saved.lastRunID, runID)
        XCTAssertNotNil(saved.lastFiredAt)
        XCTAssertEqual(saved.lastOutcome, "started run \(runID)")

        // …and it is not due again a second later: the slot was used.
        XCTAssertTrue(routines.due(at: Date().addingTimeInterval(1)).isEmpty)

        // Stop it here as well as in teardown, so the stop is *asserted* rather
        // than merely attempted.
        let outcome = await bots.stop("ea")
        XCTAssertEqual(outcome?.stopped, true, "the Space did not confirm the run died")
        print("SCHEDULER PROOF: run \(runID) stopped, confirmed by the Space "
              + "(alive=\(String(describing: outcome?.alive)))")
    }

    /// The background loop — not just a hand-driven `tick` — reaches the Space.
    @Test func testTheBackgroundSchedulerLoopFiresWithoutBeingTicked() async throws {
        await bots.connect()
        recordBaseline()
        let createdAt = Date().addingTimeInterval(-120)
        let routine = routines.create(
            botID: "inbox", title: "OpenKoalaBots live loop proof",
            prompt: "Print the single word LOOPED and exit.",
            schedule: .everyMinutes(1), now: createdAt)

        routines.startScheduler(every: .seconds(1))
        // The scheduler can fire more than once while this test waits, so the
        // teardown has to adopt every run it started, not just the last id the
        // routine happens to be holding.
        defer { routines.stopScheduler() }

        var runID: String?
        for _ in 0..<30 {
            if let id = routines.routine(routine.id)?.lastRunID { runID = id; break }
            try? await Task.sleep(nanoseconds: 1_000_000_000)
        }
        await adoptNewRuns()
        let id = try XCTUnwrap(runID, "the background loop never fired the routine")
        let live = try await liveRunIDs()
        XCTAssertTrue(live.contains(id), "run \(id) is not in agent_list: \(live)")
        print("SCHEDULER PROOF: background loop started run \(id) in \(space!)")

        let outcome = await bots.stop("inbox")
        XCTAssertEqual(outcome?.stopped, true)
    }

    /// A group chat over the live Space: two Bots, two real agent threads, one
    /// merged transcript, both stopped afterwards.
    @Test func testAGroupChatFansOutToRealAgentThreads() async throws {
        await bots.connect()
        recordBaseline()
        let groups = GroupChatStore(messenger: BotStoreGroupMessenger(store: bots))
        let chat = try groups.create(title: "OpenKoalaBots live group",
                                     members: ["sales", "talent"])

        let deliveries = await groups.send(
            "Print the single word GROUPED and exit.", in: chat.id)
        // Every run the fan-out started, including any whose id the store
        // never recorded against a bot.
        await adoptNewRuns()
        XCTAssertEqual(deliveries.count, 2)
        XCTAssertTrue(deliveries.allSatisfy(\.accepted), "\(deliveries)")

        let live = try await liveRunIDs()
        for id in chat.memberIDs {
            let run = try XCTUnwrap(bots.runID(for: id))
            XCTAssertTrue(live.contains(run), "\(id)'s run \(run) is not in agent_list")
        }
        print("GROUP PROOF: \(chat.membershipLabel) fanned out to runs "
              + "\(chat.memberIDs.compactMap { bots.runID(for: $0) }) in \(space!)")

        for id in chat.memberIDs {
            let outcome = await bots.stop(id)
            XCTAssertEqual(outcome?.stopped, true, "\(id) did not stop")
        }
    }
}
