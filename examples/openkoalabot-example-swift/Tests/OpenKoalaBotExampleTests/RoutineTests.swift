// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpaces
import Foundation
import Testing
@testable import OpenKoalaBotExample

/// Routines: the clock, persistence, and firing.
///
/// These are offline by design — the scheduler's contract is *when* it fires,
/// and asserting that against a real Space would mean waiting on wall-clock
/// time. That a firing reaches a real `agent_start` is proved separately, in
/// `RoutinesLiveTests`, against `local:cua-space-e3c1b54907`.
@MainActor
@Suite final class RoutineTests {

    /// A fresh store file per test (swift-testing makes a new suite instance
    /// for every test, so `init` is XCTest's `setUpWithError`).
    private let file: URL = URL(fileURLWithPath: NSTemporaryDirectory())
        .appendingPathComponent("openkoalabots-routines-\(UUID().uuidString).json")

    deinit {
        try? FileManager.default.removeItem(at: file)
    }

    /// A runner that records what it was asked to fire and answers however the
    /// test wants, so refusal handling is testable without a busy Bot.
    final class RecordingRunner: RoutineRunner {
        var fired: [Routine] = []
        var answer: (Routine) -> RoutineFiring = { _ in .started(runID: "run-test") }
        func fire(_ routine: Routine) async -> RoutineFiring {
            fired.append(routine)
            return answer(routine)
        }
    }

    private func date(_ iso: String) -> Date {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime]
        return f.date(from: iso)!
    }

    // MARK: - Schedule arithmetic

    @Test func testIntervalScheduleFiresOneIntervalAfterTheReference() {
        let s = RoutineSchedule.everyMinutes(30)
        let base = date("2026-09-19T08:00:00Z")
        XCTAssertEqual(s.nextFireDate(after: base), base.addingTimeInterval(1800))
    }

    @Test func testDailyScheduleRollsToTomorrowWhenTodaysSlotHasPassed() {
        var cal = Calendar(identifier: .gregorian)
        cal.timeZone = TimeZone(identifier: "UTC")!
        let s = RoutineSchedule.dailyAt(hour: 8, minute: 0)
        let afterSlot = date("2026-09-19T09:00:00Z")
        let next = s.nextFireDate(after: afterSlot, calendar: cal)
        XCTAssertEqual(next, date("2026-09-20T08:00:00Z"))
    }

    @Test func testWeeklyScheduleLandsOnTheNamedWeekday() {
        var cal = Calendar(identifier: .gregorian)
        cal.timeZone = TimeZone(identifier: "UTC")!
        // 2026-09-19 is a Saturday; the next Monday (weekday 2) is the 21st.
        let s = RoutineSchedule.weeklyOn(weekday: 2, hour: 9, minute: 30)
        let next = s.nextFireDate(after: date("2026-09-19T12:00:00Z"), calendar: cal)
        XCTAssertEqual(next, date("2026-09-21T09:30:00Z"))
        XCTAssertEqual(cal.component(.weekday, from: next!), 2)
    }

    @Test func testScheduleLabelsAreHumanReadable() {
        XCTAssertEqual(RoutineSchedule.everyMinutes(1).label, "Every minute")
        XCTAssertEqual(RoutineSchedule.everyMinutes(45).label, "Every 45 minutes")
        XCTAssertEqual(RoutineSchedule.everyMinutes(120).label, "Every 2 hours")
        XCTAssertEqual(RoutineSchedule.dailyAt(hour: 8, minute: 0).label,
                       "Every day at 8:00 AM")
        XCTAssertEqual(RoutineSchedule.dailyAt(hour: 0, minute: 5).label,
                       "Every day at 12:05 AM")
        XCTAssertEqual(RoutineSchedule.weeklyOn(weekday: 2, hour: 17, minute: 30).label,
                       "Every Monday at 5:30 PM")
    }

    @Test func testADisabledRoutineHasNoNextFireDateAndIsNeverDue() {
        var r = Routine(botID: "cos", title: "t", prompt: "p",
                        schedule: .everyMinutes(1), isEnabled: false)
        r.createdAt = date("2026-09-19T08:00:00Z")
        XCTAssertNil(r.nextFireDate(after: r.createdAt))
        XCTAssertFalse(r.isDue(at: date("2026-09-19T23:00:00Z")))
    }

    /// A scheduler that slept through six slots must fire **once** on waking,
    /// not six times into one Bot.
    @Test func testAMissedBacklogCollapsesToASingleFiring() async {
        let store = RoutineStore(fileURL: file)
        let runner = RecordingRunner()
        store.attach(runner: runner)
        let start = date("2026-09-19T08:00:00Z")
        var r = store.create(botID: "cos", title: "Hourly", prompt: "check",
                             schedule: .everyMinutes(60), now: start)
        r.lastFiredAt = start
        store.update(r)

        let sixHoursLater = start.addingTimeInterval(6 * 3600)
        let fired = await store.tick(now: sixHoursLater)
        XCTAssertEqual(fired.count, 1)
        XCTAssertEqual(runner.fired.count, 1, "a slept-through backlog must not stack up")
    }

    // MARK: - Persistence

    /// The defining property: a routine survives the app being killed.
    @Test func testARoutineSurvivesARestart() {
        let first = RoutineStore(fileURL: file)
        let created = first.create(botID: "inbox", title: "8am triage",
                                   prompt: "Triage the inbox and draft replies.",
                                   schedule: .dailyAt(hour: 8, minute: 0))

        // A brand-new store over the same file is exactly what a relaunch is.
        let second = RoutineStore(fileURL: file)
        XCTAssertEqual(second.routines.count, 1)
        let reloaded = second.routine(created.id)
        XCTAssertEqual(reloaded?.title, "8am triage")
        XCTAssertEqual(reloaded?.prompt, "Triage the inbox and draft replies.")
        XCTAssertEqual(reloaded?.schedule, .dailyAt(hour: 8, minute: 0))
        XCTAssertEqual(reloaded?.botID, "inbox")
        XCTAssertEqual(reloaded?.isEnabled, true)
    }

    @Test func testEveryScheduleShapeRoundTripsThroughDisk() throws {
        let store = RoutineStore(fileURL: file)
        store.create(botID: "cos", title: "a", prompt: "p", schedule: .everyMinutes(7))
        store.create(botID: "cos", title: "b", prompt: "p",
                     schedule: .dailyAt(hour: 6, minute: 45))
        store.create(botID: "cos", title: "c", prompt: "p",
                     schedule: .weeklyOn(weekday: 6, hour: 17, minute: 0))

        let reloaded = RoutineStore(fileURL: file)
        XCTAssertEqual(reloaded.routines.map(\.schedule),
                       [.everyMinutes(7), .dailyAt(hour: 6, minute: 45),
                        .weeklyOn(weekday: 6, hour: 17, minute: 0)])
    }

    @Test func testFiringHistorySurvivesARestartSoAReopenDoesNotRefireThePast() async {
        let store = RoutineStore(fileURL: file)
        let runner = RecordingRunner()
        store.attach(runner: runner)
        let r = store.create(botID: "cos", title: "Hourly", prompt: "p",
                             schedule: .everyMinutes(60),
                             now: date("2026-09-19T07:00:00Z"))
        await store.fire(r, now: date("2026-09-19T08:00:00Z"))

        let reopened = RoutineStore(fileURL: file)
        let after = reopened.routine(r.id)
        XCTAssertEqual(after?.lastFiredAt, date("2026-09-19T08:00:00Z"))
        XCTAssertEqual(after?.lastRunID, "run-test")
        // Ten minutes after the restart the hourly slot has not come round.
        XCTAssertTrue(reopened.due(at: date("2026-09-19T08:10:00Z")).isEmpty)
        XCTAssertEqual(reopened.due(at: date("2026-09-19T09:30:00Z")).count, 1)
    }

    @Test func testDeleteRemovesItFromDiskToo() {
        let store = RoutineStore(fileURL: file)
        let r = store.create(botID: "cos", title: "x", prompt: "p", schedule: .everyMinutes(5))
        store.delete(r.id)
        XCTAssertTrue(RoutineStore(fileURL: file).routines.isEmpty)
    }

    @Test func testDisablingPersistsAndKeepsHistory() async {
        let store = RoutineStore(fileURL: file)
        store.attach(runner: RecordingRunner())
        let r = store.create(botID: "cos", title: "x", prompt: "p", schedule: .everyMinutes(5))
        await store.fire(r, now: date("2026-09-19T08:00:00Z"))
        store.setEnabled(false, for: r.id)

        let reloaded = RoutineStore(fileURL: file)
        XCTAssertEqual(reloaded.routines.first?.isEnabled, false)
        XCTAssertNotNil(reloaded.routines.first?.lastFiredAt,
                        "disabling must not erase a routine's history")
        XCTAssertTrue(reloaded.due(at: date("2026-09-20T08:00:00Z")).isEmpty)
    }

    // MARK: - Firing

    @Test func testTickFiresOnlyWhatIsDue() async {
        let store = RoutineStore(fileURL: file)
        let runner = RecordingRunner()
        store.attach(runner: runner)
        let start = date("2026-09-19T08:00:00Z")
        store.create(botID: "cos", title: "soon", prompt: "p",
                     schedule: .everyMinutes(5), now: start)
        store.create(botID: "ea", title: "later", prompt: "p",
                     schedule: .everyMinutes(600), now: start)

        let fired = await store.tick(now: start.addingTimeInterval(6 * 60))
        XCTAssertEqual(fired.map(\.title), ["soon"])
        XCTAssertEqual(runner.fired.map(\.title), ["soon"])
    }

    /// A refusal is recorded as a refusal, not laundered into a success and
    /// not dropped. `FRICTION.md` #24 is the reason this matters.
    @Test func testARefusedFiringIsRecordedAndDoesNotRetryEveryTick() async {
        let store = RoutineStore(fileURL: file)
        let runner = RecordingRunner()
        runner.answer = { _ in .refused(reason: "mid-turn") }
        store.attach(runner: runner)
        let start = date("2026-09-19T08:00:00Z")
        store.create(botID: "cos", title: "busy", prompt: "p",
                     schedule: .everyMinutes(60), now: start)

        let fired = await store.tick(now: start.addingTimeInterval(3700))
        XCTAssertEqual(fired.count, 1)
        XCTAssertEqual(fired[0].firing, .refused(reason: "mid-turn"))
        XCTAssertEqual(store.routines[0].lastOutcome, "refused: mid-turn")
        XCTAssertNil(store.routines[0].lastRunID, "a refusal did not produce a run")
        XCTAssertEqual(store.log.first?.firing, .refused(reason: "mid-turn"))

        // One minute later the slot is used up, so nothing fires again.
        let again = await store.tick(now: start.addingTimeInterval(3760))
        XCTAssertTrue(again.isEmpty)
    }

    @Test func testFiringWithNoRunnerFailsLoudlyRatherThanSilently() async {
        let store = RoutineStore(fileURL: file)
        let r = store.create(botID: "cos", title: "x", prompt: "p", schedule: .everyMinutes(1))
        let record = await store.fire(r)
        guard case .failed(let reason) = record.firing else {
            return XCTFail("expected a failure, got \(record.firing)")
        }
        XCTAssertTrue(reason.contains("no runner"))
    }

    /// The live runner's decision table, without a Space: a busy Bot is never
    /// interrupted by a routine.
    @Test func testTheLiveRunnerRefusesRatherThanInterruptingARunningBot() async {
        let store = BotStore(client: DemoSpacesClient(), identities: Fixtures.bots)
        await store.connect()
        // Hiring through the demo client leaves the Bot `running`.
        _ = try? await store.hire("cos", prompt: "seed")
        XCTAssertEqual(store.presence(for: "cos").state, .running)

        let runner = BotStoreRoutineRunner(store: store)
        let firing = await runner.fire(Routine(botID: "cos", title: "t", prompt: "p",
                                               schedule: .everyMinutes(5)))
        guard case .refused(let reason) = firing else {
            return XCTFail("a routine must not interrupt a Bot mid-turn; got \(firing)")
        }
        XCTAssertTrue(reason.contains("mid-turn"))
    }

    @Test func testTheLiveRunnerHiresAnUnhiredBotSoAFiringIsARealRun() async {
        let store = BotStore(client: DemoSpacesClient(), identities: Fixtures.bots)
        await store.connect()
        let runner = BotStoreRoutineRunner(store: store)
        let firing = await runner.fire(Routine(botID: "ea", title: "Daily brief",
                                               prompt: "Summarise the calendar.",
                                               schedule: .dailyAt(hour: 8, minute: 0)))
        XCTAssertEqual(firing.runID, "run-ea")
        XCTAssertEqual(store.runID(for: "ea"), "run-ea")
    }

    /// Scheduled work is marked in the transcript, so a user can tell it from
    /// something they typed.
    @Test func testARoutineTurnIsMarkedInTheTranscript() async {
        let store = BotStore(client: DemoSpacesClient(), identities: Fixtures.bots)
        await store.connect()
        let runner = BotStoreRoutineRunner(store: store)
        _ = await runner.fire(Routine(botID: "ea", title: "Daily brief",
                                      prompt: "Summarise the calendar.",
                                      schedule: .everyMinutes(60)))
        let userLines = store.thread(for: "ea").messages.compactMap { m -> String? in
            guard m.sender == .user, case .prose(let t) = m.body else { return nil }
            return t
        }
        XCTAssertEqual(userLines.count, 1)
        XCTAssertTrue(userLines[0].hasPrefix(BotStoreRoutineRunner.prefix))
        XCTAssertTrue(userLines[0].contains("Daily brief"))
    }

    @Test func testTheSchedulerLoopStartsAndStops() {
        let store = RoutineStore(fileURL: file)
        XCTAssertFalse(store.isSchedulerRunning)
        store.startScheduler(every: .seconds(60))
        XCTAssertTrue(store.isSchedulerRunning)
        store.stopScheduler()
        XCTAssertFalse(store.isSchedulerRunning)
    }
}
