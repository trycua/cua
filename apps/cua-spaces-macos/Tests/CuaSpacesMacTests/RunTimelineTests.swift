// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
@testable import CuaSpacesMacKit
import Foundation
import Testing

@MainActor
@Suite("Run timeline")
struct RunTimelineTests {
    func timeline(_ runId: String) async -> RunTimelineModel {
        let backend = FixtureSpacesBackend()
        backend.fixtureRuns["local:aurora"] = FixtureSpacesBackend.sampleRuns
        backend.fixtureEvents = FixtureSpacesBackend.sampleEvents
        let runs = try! await backend.agentRuns(id: "local:aurora")
        let run = runs.first { $0.runId == runId }!
        let model = RunTimelineModel(backend: backend, spaceId: "local:aurora", run: run)
        await model.refresh()
        return model
    }

    /// The items and labels the web UI's transcript.ts gives the same events.
    @Test func aRunningRunFoldsLikeTheWebUI() async {
        let t = await timeline("run-ada-7")
        #expect(t.items.map(\.kind) == ["user", "activity", "message", "activity", "user", "activity"])
        #expect(t.items[0].text == "Fix the flaky login test in the web app")
        // A tool's update rewrites its own step; only `error` events count as errors.
        #expect(t.items[1].text == "4 steps")
        #expect(t.items[1].steps == [
            "Thinking: The test waits on a fixed timeout.",
            "Tool rg -n \"login\" tests/ completed",
            "Tool Read tests/login.spec.ts completed",
            "Tool pnpm test login failed: 1 failed: timed out after 5000 ms",
        ])
        // Message chunks of a turn join into one item.
        #expect(t.items[2].text == "The login test waits a fixed **5 s** for the redirect. I replaced it with `waitForURL`, so it waits exactly as long as the redirect takes.")
        #expect(t.items[3].steps == ["Turn 1 ended (end_turn)"])
        #expect(t.items[5].text == "1 step")
        #expect(t.items[5].steps == ["Tool pnpm test in_progress"])
        // The newest group stays open while its turn runs.
        #expect(t.openActivity == 5)
        #expect(t.footer?.text == "Using tools")
        #expect(t.error == nil)
    }

    @Test func aFailedRunCountsItsErrorAndSaysWhy() async {
        let t = await timeline("run-codex-2")
        #expect(t.items.map(\.kind) == ["user", "activity"])
        #expect(t.items[1].text == "2 steps, 1 error")
        #expect(t.items[1].steps == ["Error: auth failed: no OpenAI key", "Exited: exit status 1"])
        #expect(t.openActivity == nil)
        #expect(t.footer?.text == "Failed: auth failed")
        #expect(t.footer?.failed == true)
    }

    @Test func readingAgainAddsNothing() async {
        let t = await timeline("run-ada-7")
        let before = t.items
        await t.refresh()
        #expect(t.items == before)
    }

    @Test func aFailedReadIsAnError() async {
        let backend = FixtureSpacesBackend()
        backend.fixtureRuns["local:aurora"] = FixtureSpacesBackend.sampleRuns
        let run = try! await backend.agentRuns(id: "local:aurora")[0]
        let t = RunTimelineModel(backend: backend, spaceId: "local:aurora", run: run)
        await t.refresh()
        #expect(t.items.isEmpty)
        #expect(t.error != nil)
        #expect(!t.loading)
    }
}
