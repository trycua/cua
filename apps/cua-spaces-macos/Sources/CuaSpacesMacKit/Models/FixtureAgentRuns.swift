// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation

/// The fixture Space's coding-agent runs and what they wrote
/// (`CUA_SPACES_FIXTURES=1`): the Agents section and a run's conversation.
extension FixtureSpacesBackend {
    /// `agent_list` records for `local:aurora`.
    public static let sampleRuns = [
        #"{"run_id":"run-ada-7","harness":"claude-code","status":"running","phase":"tool","reason":"a turn is running","turn":2,"accepts_message":false,"meta":{"run_id":"run-ada-7","harness":"claude-code","prompt":"Fix the flaky login test in the web app","cwd":"/home/cua/app","created_at":1791800000.0}}"#,
        #"{"run_id":"run-codex-2","harness":"openai-codex","status":"failed","phase":"failed","reason":"auth failed","turn":1,"accepts_message":false,"meta":{"run_id":"run-codex-2","harness":"openai-codex","prompt":"Triage the open issues","cwd":"/home/cua","created_at":1791790000.0}}"#,
    ]

    /// `agent_events` pages per run id: every event, caught up.
    public static let sampleEvents: [String: String] = [
        "run-ada-7": page(runId: "run-ada-7", status: "running", phase: "tool", events: [
            ["kind": "turn_started", "turn": 1, "text": "Fix the flaky login test in the web app"],
            ["kind": "thought", "turn": 1, "text": "The test waits on a fixed timeout.\nLook at the spec first."],
            ["kind": "tool_call", "turn": 1, "tool_id": "t1", "tool_title": "rg -n \"login\" tests/", "tool_kind": "execute"],
            ["kind": "tool_update", "turn": 1, "tool_id": "t1", "tool_status": "completed"],
            ["kind": "tool_call", "turn": 1, "tool_id": "t2", "tool_title": "Read tests/login.spec.ts", "tool_kind": "read"],
            ["kind": "tool_update", "turn": 1, "tool_id": "t2", "tool_status": "completed"],
            ["kind": "tool_call", "turn": 1, "tool_id": "t3", "tool_title": "pnpm test login", "tool_kind": "execute"],
            ["kind": "tool_update", "turn": 1, "tool_id": "t3", "tool_status": "failed", "text": "1 failed: timed out after 5000 ms"],
            ["kind": "message", "turn": 1, "text": "The login test waits a fixed **5 s** for the redirect. "],
            ["kind": "message", "turn": 1, "text": "I replaced it with `waitForURL`, so it waits exactly as long as the redirect takes."],
            ["kind": "turn_ended", "turn": 1, "stop_reason": "end_turn"],
            ["kind": "turn_started", "turn": 2, "text": "Run the whole suite to be sure"],
            ["kind": "tool_call", "turn": 2, "tool_id": "t4", "tool_title": "pnpm test", "tool_kind": "execute"],
            ["kind": "tool_update", "turn": 2, "tool_id": "t4", "tool_status": "in_progress"],
        ]),
        "run-codex-2": page(runId: "run-codex-2", status: "failed", phase: "failed", events: [
            ["kind": "turn_started", "turn": 1, "text": "Triage the open issues"],
            ["kind": "error", "turn": 1, "text": "auth failed: no OpenAI key"],
            ["kind": "exited", "turn": 1, "text": "exit status 1"],
        ]),
    ]

    private static func page(runId: String, status: String, phase: String, events: [[String: Any]]) -> String {
        let t0: UInt64 = 1_791_800_000_000
        let numbered = events.enumerated().map { i, e in
            e.merging(["seq": i + 1, "ts_ms": t0 + UInt64(i) * 4000]) { a, _ in a }
        }
        let page: [String: Any] = ["run_id": runId, "status": status, "phase": phase, "events": numbered,
                                   "cursor": numbered.count, "caught_up": true]
        let data = (try? JSONSerialization.data(withJSONObject: page, options: [.sortedKeys])) ?? Data()
        return String(decoding: data, as: UTF8.self)
    }
}
