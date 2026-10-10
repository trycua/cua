// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSpacesFFI
@testable import CuaSpacesMacKit
import Foundation
import SwiftUI
import Testing

/// An in-memory daemon for Settings → Agents: `agent_keys.*` as the daemon
/// answers them (never a value).
final class FakeAgentKeysTools: AgentsToolRunning, @unchecked Sendable {
    var calls: [(String, [String: Any])] = []
    var keys: [[String: Any]] = [["provider": "openai", "env": "OPENAI_API_KEY", "last4": "3f9a", "added_ms": 1_790_000_000_000]]
    var available = true

    func agentsTool(_ tool: String, _ args: [String: Any]) async throws -> Any {
        calls.append((tool, args))
        switch tool {
        case "agent_keys.list": break
        case "agent_keys.set":
            let provider = args["provider"] as? String ?? ""
            let env = provider == "anthropic" ? "ANTHROPIC_API_KEY" : provider == "openai" ? "OPENAI_API_KEY" : args["env"] as? String ?? ""
            let value = args["value"] as? String ?? ""
            keys.removeAll { $0["env"] as? String == env }
            keys.append(["provider": provider, "env": env, "last4": String(value.suffix(4)), "added_ms": 1_790_000_100_000])
        case "agent_keys.remove":
            keys.removeAll { $0["env"] as? String == args["env"] as? String }
        default: throw AgentsToolError(message: "unknown tool \(tool)")
        }
        var report: [String: Any] = ["keys": keys, "providers": [], "available": available]
        if !available { report["unavailable"] = "this build keeps credentials in a file" }
        return report
    }
}

@MainActor
@Suite("Settings → Agents")
struct AgentKeysTests {
    init() { _ = NSApplication.shared }

    @Test func savesAndRemovesKeysAndNeverKeepsTheValue() async throws {
        let tools = FakeAgentKeysTools()
        let m = AgentKeysModel(tools: tools)
        await m.load()
        #expect(m.view.rows.map(\.status) == ["Not set", "\u{2022}\u{2022}\u{2022}\u{2022} 3f9a"])
        #expect(m.view.canEdit)
        #expect(m.form(provider: "anthropic", env: nil, name: "", hasValue: false).canSave == false)
        try await m.save(provider: "anthropic", env: "ANTHROPIC_API_KEY", value: "sk-ant-test-0000")
        // A provider key goes without a variable name; the daemon picks it.
        #expect(tools.calls.last?.0 == "agent_keys.set")
        #expect(tools.calls.last?.1["env"] == nil)
        #expect(m.view.rows[0].status == "\u{2022}\u{2022}\u{2022}\u{2022} 0000")
        #expect(m.view.rows[0].removeLabel == "Remove")
        #expect(m.removeConfirm(env: "ANTHROPIC_API_KEY")?.title == "Remove the Anthropic key?")
        #expect(!String(describing: m.report ?? [:]).contains("sk-ant-test"))
        try await m.save(provider: "other", env: "MISTRAL_API_KEY", value: "mk-0000-9z9z")
        #expect(tools.calls.last?.1["env"] as? String == "MISTRAL_API_KEY")
        #expect(m.view.rows.map(\.title) == ["Anthropic", "OpenAI", "MISTRAL_API_KEY"])
        try await m.remove(env: "ANTHROPIC_API_KEY")
        #expect(m.view.rows[0].status == "Not set")
    }

    @Test func otherNamesAreCheckedByTheCore() {
        let m = AgentKeysModel(tools: FakeAgentKeysTools())
        let bad = m.form(provider: "other", env: nil, name: "DYLD_INSERT_LIBRARIES", hasValue: true)
        #expect(!bad.canSave)
        #expect(bad.nameError?.contains("changes how programs run") == true)
        let ok = m.form(provider: "other", env: nil, name: "GEMINI_API_KEY", hasValue: true)
        #expect(ok.canSave && ok.env == "GEMINI_API_KEY")
    }

    @Test func saysWhyKeysCantBeSaved() async {
        let tools = FakeAgentKeysTools()
        tools.available = false
        let m = AgentKeysModel(tools: tools)
        await m.load()
        #expect(!m.view.canEdit)
        #expect(m.view.notice?.contains("keeps credentials in a file") == true)
        let none = AgentKeysModel(tools: nil)
        #expect(none.view.notice?.contains("need the cua daemon") == true)
    }

    @Test func theSettingsTabRenders() async {
        let m = AgentKeysModel(tools: FakeAgentKeysTools())
        await m.load()
        let host = NSHostingView(rootView: AgentKeysSettingsView(keys: m))
        host.frame = NSRect(x: 0, y: 0, width: 520, height: 420)
        host.layoutSubtreeIfNeeded()
        #expect(host.fittingSize.width > 0)
        #expect(AgentKeysSettingsView.added("Added", 1_790_000_000_000).hasPrefix("Added "))
    }

    @Test func theWebBridgeRoutesAgentKeysToTheDaemon() {
        for m in ["agentKeys.list", "agentKeys.set", "agentKeys.remove"] {
            #expect(WebUIBridge.methods.contains(m))
        }
    }
}
