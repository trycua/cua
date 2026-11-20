// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation

/// The risky-action question's files (`cua config set nudges.risky_actions
/// on`): `cua mcp` writes `$CUA_HOME/nudges/<id>.json` and waits for
/// `<id>.answer` (`separate` or `here`); with no answer in time it runs the
/// call here (`libs/cua/crates/cua-cli/src/risky.rs`). The web page asks
/// through `nudges.pending` and `nudges.answer` and draws the question.
struct NudgeRequests {
    let dir: URL

    init(home: URL = NudgeRequests.cuaHome) {
        dir = home.appendingPathComponent("nudges", isDirectory: true)
    }

    /// The cua home `cua mcp` uses: `CUA_HOME`, else `~/.cua`
    /// (`~/.cua/envs/staging` under `CUA_ENV=staging`, as `cua_home()`).
    static var cuaHome: URL {
        let env = ProcessInfo.processInfo.environment
        if let home = env["CUA_HOME"], !home.isEmpty { return URL(fileURLWithPath: home, isDirectory: true) }
        let base = URL(fileURLWithPath: env["HOME"] ?? NSHomeDirectory(), isDirectory: true)
            .appendingPathComponent(".cua", isDirectory: true)
        let staging = (env["CUA_ENV"] ?? "").trimmingCharacters(in: .whitespaces).lowercased() == "staging"
        return staging ? base.appendingPathComponent("envs/staging", isDirectory: true) : base
    }

    /// An id as `cua mcp` writes it, so no path can hide in one.
    static func validId(_ id: String) -> Bool {
        id.range(of: #"^risky-[0-9]+-[0-9]+$"#, options: .regularExpression) != nil
    }

    /// Requests still waiting (no answer, not past their deadline), oldest
    /// first, as the page reads them (`RiskyRequest`).
    func pending(nowMs: Double = Date().timeIntervalSince1970 * 1000) -> [[String: Any]] {
        let names = (try? FileManager.default.contentsOfDirectory(atPath: dir.path)) ?? []
        return names.compactMap { name -> [String: Any]? in
            guard name.hasSuffix(".json") else { return nil }
            let id = String(name.dropLast(5))
            guard Self.validId(id),
                  !FileManager.default.fileExists(atPath: dir.appendingPathComponent("\(id).answer").path),
                  let data = try? Data(contentsOf: dir.appendingPathComponent(name)),
                  let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                  let expires = (obj["expiresMs"] as? NSNumber)?.doubleValue, expires > nowMs
            else { return nil }
            return ["id": id, "agent": obj["agent"] as? String ?? "", "action": obj["action"] as? String ?? "",
                    "atMs": (obj["atMs"] as? NSNumber)?.doubleValue ?? 0, "expiresMs": expires]
        }
        .sorted { ($0["atMs"] as? Double ?? 0) < ($1["atMs"] as? Double ?? 0) }
    }

    /// Answers a waiting request. One that is gone (it ran here already)
    /// is left alone.
    func answer(id: String, choice: String) throws {
        guard Self.validId(id), choice == "separate" || choice == "here" else { return }
        guard FileManager.default.fileExists(atPath: dir.appendingPathComponent("\(id).json").path) else { return }
        try Data(choice.utf8).write(to: dir.appendingPathComponent("\(id).answer"), options: .atomic)
    }
}

extension WebUIBridge {
    func routeNudges(_ method: String, _ args: [String: Any]) throws -> Any? {
        switch method {
        case "nudges.pending":
            return NudgeRequests().pending()
        case "nudges.answer":
            guard let id = args["id"] as? String, let choice = args["choice"] as? String else {
                throw Failure.badArgs("id: string, choice: separate or here")
            }
            try NudgeRequests().answer(id: id, choice: choice)
            return NSNull()
        default:
            return nil
        }
    }
}
