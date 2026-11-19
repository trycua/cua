// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
@testable import CuaSpacesMacKit
import Foundation
import Testing

@MainActor
@Suite("Agent runs")
struct AgentRunsTests {
    static let runs = [
        #"{"run_id":"r-run","harness":"claude-code","status":"running","phase":"working","reason":"a turn is running","turn":1,"accepts_message":false,"meta":{"run_id":"r-run","harness":"claude-code","prompt":"fix\n the build","cwd":"/root","created_at":20.0}}"#,
        #"{"run_id":"r-fail","harness":null,"status":"failed","phase":"failed","reason":"auth failed","turn":0,"accepts_message":false,"meta":{"run_id":"r-fail","harness":"openai-codex","prompt":"triage","cwd":"/root","created_at":10.0}}"#,
        #"{"run_id":"r-bad","status":"unknown","phase":"unknown","reason":"record unreadable","turn":0,"accepts_message":false}"#,
    ]

    @Test func agentRunsAreTheCoresRowsAttentionFirst() async {
        let backend = FixtureSpacesBackend()
        backend.fixtureRuns["local:aurora"] = Self.runs
        let agents = AgentRunsModel(backend: backend, spaceId: "local:aurora")
        #expect(agents.load == .loading)
        await agents.refresh()
        guard case .ready(let rows) = agents.load else {
            Issue.record("not ready: \(agents.load)")
            return
        }
        #expect(rows.map(\.runId) == ["r-fail", "r-run", "r-bad"])
        #expect(AgentRunsModel.line(rows[0]) == "OpenAI Codex \u{b7} triage")
        #expect(AgentRunsModel.line(rows[1]) == "Claude Code \u{b7} fix the build")
        #expect(AgentRunsModel.status(rows[1]) == "Running")
        // A record that could not be read is still a row, status Unknown.
        #expect(AgentRunsModel.line(rows[2]) == "Unknown agent \u{b7} this run's record could not be read")
        #expect(AgentRunsModel.status(rows[2]) == "Unknown")
    }

    @Test func aFailedReadIsNotAnEmptyList() async {
        let backend = FixtureSpacesBackend()
        backend.agentRunsError = TimeoutError.timedOut
        let agents = AgentRunsModel(backend: backend, spaceId: "local:aurora")
        await agents.refresh()
        guard case .failed = agents.load else {
            Issue.record("expected failed, got \(agents.load)")
            return
        }
        backend.agentRunsError = nil
        await agents.refresh()
        #expect(agents.load == .ready([]))
    }
}
