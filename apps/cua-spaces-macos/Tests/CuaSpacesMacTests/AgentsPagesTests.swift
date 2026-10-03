// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
@testable import CuaSpacesMacKit
import Foundation
import SwiftUI
import Testing

/// An in-memory daemon for the Agents, Drive and Notifications pages.
final class FakeAgentsTools: AgentsToolRunning, @unchecked Sendable {
    var calls: [String] = []
    var paused = false
    var grants: [[String: Any]] = []
    let now: Int64 = 1_790_000_000_000

    func agentsTool(_ tool: String, _ args: [String: Any]) async throws -> Any {
        calls.append(tool)
        switch tool {
        case "persistent_agent_list":
            return ["agents": [
                ["name": "ada", "harness": "hermes", "space": "local:dev", "paused": paused, "run_id": paused ? NSNull() : "run-1", "saved_ms": now - 120_000],
                ["name": "scout", "harness": "claude-code", "space": "cloud:scout", "paused": true, "space_state": "released", "saved_ms": now - 7_200_000],
            ]]
        case "agent_pause": paused = true; return [:]
        case "volume_ls":
            let path = args["path"] as? String ?? ""
            if path == "agents/ada/" { return ["entries": [["path": "agents/ada/hermes/", "name": "hermes", "folder": true]]] }
            if path == "agents/ada/hermes/" { return ["entries": [["path": "agents/ada/hermes/MEMORY.md", "name": "MEMORY.md", "folder": false, "size": 1830]]] }
            return ["entries": [["path": "agents/", "name": "agents", "folder": true], ["path": "public/", "name": "public", "folder": true],
                                ["path": "spaces/", "name": "spaces", "folder": true]]]
        case "routine_list":
            return ["routines": [["id": "R1", "botID": "ada", "title": "Morning", "label": "Every day at 8:00 AM", "isEnabled": true]]]
        case "computer_access_list": return ["grants": grants, "audit": []]
        case "computer_access_grant":
            grants = [["agent": "ada", "machine": args["machine"] ?? "", "revoked": false]]; return grants[0]
        case "volume_requests":
            return ["requests": [["id": "q1", "principal": "agent:ada", "prefix": "agents/writer/outputs/", "mode": "r", "reason": "cite the draft"]]]
        case "volume_grants": return ["grants": []]
        case "notifications_list":
            return ["notifications": [["id": "n1", "at_ms": now - 60_000, "agent": "ada", "kind": "turn_ended", "title": "ada", "body": "Your research is ready.", "read": false]]]
        default: throw AgentsToolError(message: "unknown tool \(tool)")
        }
    }
}

@MainActor
@Suite("Agents pages", .serialized)
struct AgentsPagesTests {
    init() { _ = NSApplication.shared }

    @Test func pauseAllowAndNotificationsFlowThroughTheCore() async throws {
        let tools = FakeAgentsTools()
        let m = PersistentModel(tools: tools)
        m.thisMachine = "relay:0123"
        await m.loadAgents()
        #expect(m.agentsView().rows.map(\.state) == ["Running", "Paused"])
        await m.send(.pause(name: "ada"))
        #expect(tools.calls.contains("agent_pause"))
        #expect(m.agentsView().rows[0].state == "Paused")
        await m.send(.select(name: "ada"))
        #expect(m.agentsView().detail?.memory.map(\.text) == ["hermes/MEMORY.md"])
        await m.send(.allowComputer(machine: "relay:0123"))
        #expect(m.agentsView().detail?.access.map(\.text) == ["This computer"])

        var posted: [AppSystemNote] = []
        var seen: UInt64 = 0
        m.post = { posted.append($0) }
        m.seenMs = { seen }
        m.saveSeenMs = { seen = $0 }
        await m.pollNotifications()
        #expect(posted.isEmpty, "the backlog is not posted on the first run")
        #expect(seen == UInt64(tools.now - 60_000))
    }

    @Test func snapshots() async throws {
        let tools = FakeAgentsTools()
        let m = PersistentModel(tools: tools)
        m.now = { Date(timeIntervalSince1970: TimeInterval(tools.now) / 1000) }
        m.thisMachine = "relay:0123"
        await m.loadAgents()
        await m.send(.select(name: "ada"))
        let size = CGSize(width: 720, height: 520)
        try SnapshotTests().assertSnapshot(AgentsPageView(model: m), "agents-page", size: size)
        await m.sendDrive(nil)
        try SnapshotTests().assertSnapshot(DrivePageView(model: m), "drive-page", size: size)
        await m.pollNotifications()
        try SnapshotTests().assertSnapshot(NotificationsPageView(model: m), "notifications-page", size: size)
    }
}
