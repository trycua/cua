// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation

/// What the person approved for an agent home while it runs (CUA-2630): see
/// every Space, use a machine. Written to `<cua home>/agent-home/grants.json`,
/// which this machine's cua daemon reads each time the home asks
/// (`children/home.rs`), so an approval takes effect at once with no restart.
/// The file is outside the home Space: the agent in it can never write it.
struct HomeGrants: Equatable, Sendable {
    var listAll: Bool?
    var machines: [String]?
    /// Spaces and machines @mentioned in a later request (ids and names): an @mention counts as the grant.
    var mentions: [String]?

    /// The grants in a `spaces.setAccess` request's `home`; the wrong types are ignored.
    static func parse(_ value: Any?) -> HomeGrants? {
        guard let o = value as? [String: Any] else { return nil }
        var g = HomeGrants()
        if let l = o["listAll"] as? Bool { g.listAll = l }
        if let m = o["machines"] as? [Any] {
            g.machines = Array(Set(m.compactMap { $0 as? String }.filter { !$0.isEmpty })).sorted()
        }
        if let m = o["mentions"] as? [Any] {
            g.mentions = Array(Set(m.compactMap { $0 as? String }.filter { !$0.isEmpty })).sorted()
        }
        return g.listAll == nil && g.machines == nil && g.mentions == nil ? nil : g
    }

    /// The cua home the daemon uses (`CUA_HOME`, else `~/.cua`).
    static func cuaHome(_ env: [String: String] = ProcessInfo.processInfo.environment) -> URL {
        if let h = env["CUA_HOME"], !h.isEmpty { return URL(fileURLWithPath: h, isDirectory: true) }
        return URL(fileURLWithPath: env["HOME"] ?? NSHomeDirectory(), isDirectory: true).appendingPathComponent(".cua", isDirectory: true)
    }

    /// Sets `space`'s grants in the file, keeping the other homes'.
    static func write(home: URL, space: String, _ g: HomeGrants) throws {
        let dir = home.appendingPathComponent("agent-home", isDirectory: true)
        let file = dir.appendingPathComponent("grants.json")
        var all: [String: Any] = [:]
        if let data = try? Data(contentsOf: file), let parsed = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] {
            all = parsed
        }
        let prev = all[space] as? [String: Any] ?? [:]
        // Mentions only accumulate: a later request never forgets what an earlier one named.
        let mentions = Array(Set((prev["mentions"] as? [String] ?? []) + (g.mentions ?? []))).sorted()
        var entry: [String: Any] = [
            "list_all": g.listAll ?? (prev["list_all"] as? Bool) ?? false,
            "machines": g.machines ?? (prev["machines"] as? [String]) ?? [],
        ]
        if !mentions.isEmpty { entry["mentions"] = mentions }
        all[space] = entry
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        let data = try JSONSerialization.data(withJSONObject: all, options: [.sortedKeys])
        let tmp = dir.appendingPathComponent("grants.json.\(ProcessInfo.processInfo.processIdentifier).tmp")
        try data.write(to: tmp)
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: tmp.path)
        _ = try FileManager.default.replaceItemAt(file, withItemAt: tmp)
    }
}
