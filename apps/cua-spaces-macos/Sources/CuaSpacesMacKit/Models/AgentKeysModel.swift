// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpacesFFI
import Foundation
import Observation

/// Settings → Agents: the provider keys agents get. The cua daemon keeps
/// them in the Keychain (`agent_keys.list/set/remove`, its app methods; no
/// MCP tool reaches them) and answers only the provider, the variable, the
/// last four characters and when each was added. Every word and rule is the
/// app core's (`appAgentKeysView`, `appAgentKeyForm`); the web UI's
/// Settings → Agents draws the same.
///
/// A key passes through here once, in `save`, and is never kept.
@MainActor @Observable
public final class AgentKeysModel {
    private let tools: AgentsToolRunning?
    /// The daemon's last answer, as it came.
    private(set) var report: [String: Any]?
    /// Why the keys could not be read.
    public private(set) var error: String?

    public init(tools: AgentsToolRunning?) {
        self.tools = tools
    }

    /// The daemon's methods answer here.
    public var available: Bool { tools != nil }

    /// Runs one of the daemon's `agent_keys.*` methods; its answer (the list
    /// after it) replaces the shown one, and is returned as is.
    @discardableResult
    func call(_ method: String, _ args: [String: Any] = [:]) async throws -> Any {
        guard let tools else { throw AgentsToolError(message: "Agent keys need the cua daemon") }
        let r = try await tools.agentsTool(method, args)
        if let r = r as? [String: Any] {
            report = r
            error = nil
        }
        return r
    }

    public func load() async {
        do {
            try await call("agent_keys.list")
        } catch {
            self.error = error.localizedDescription
        }
    }

    /// Adds or replaces a key (`env` names an Other key).
    public func save(provider: String, env: String?, value: String) async throws {
        var args: [String: Any] = ["provider": provider, "value": value]
        if provider == "other", let env, !env.isEmpty { args["env"] = env }
        try await call("agent_keys.set", args)
    }

    public func remove(env: String) async throws {
        try await call("agent_keys.remove", ["env": env])
    }

    /// The core's input for the daemon's answer.
    public var input: AppAgentKeysInput {
        Self.input(report: report, error: error ?? (available ? nil : "Agent keys need the cua daemon"))
    }

    static func input(report: [String: Any]?, error: String?) -> AppAgentKeysInput {
        let keys = (report?["keys"] as? [Any] ?? []).compactMap { $0 as? [String: Any] }.map { k in
            AppAgentKeyInput(provider: k["provider"] as? String ?? "other", env: k["env"] as? String ?? "",
                             last4: k["last4"] as? String ?? "",
                             addedMs: (k["added_ms"] as? NSNumber)?.uint64Value ?? 0)
        }
        let unavailable = report?["available"] as? Bool == false
            ? (report?["unavailable"] as? String ?? "this machine can't keep keys") : nil
        return AppAgentKeysInput(keys: keys.filter { !$0.env.isEmpty }, unavailable: unavailable, error: error)
    }

    public var view: AppAgentKeysView { appAgentKeysView(input: input) }

    public func form(provider: String, env: String?, name: String, hasValue: Bool) -> AppAgentKeyFormView {
        appAgentKeyForm(input: input, form: AppAgentKeyFormInput(provider: provider, env: env, name: name, hasValue: hasValue))
    }

    public func removeConfirm(env: String) -> AppAgentKeyConfirm? {
        appAgentKeyRemoveConfirm(input: input, env: env)
    }
}
