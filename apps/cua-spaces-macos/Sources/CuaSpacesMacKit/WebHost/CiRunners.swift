// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation

/// The CI Runners page's methods (`ci.*`, `ops/ci.ts` in the web bridge): each
/// runs the bundled `cua ci app <op>` with the arguments as one JSON object on
/// stdin and answers the one JSON line it prints
/// (`libs/cua/crates/cua-cli/src/ci_app.rs`). The rules (what may be asked,
/// what is checked, where a GitHub token goes) are the CLI's; this only runs
/// it, with the operation's name fixed here and each argument checked for its
/// type. A token reaches `cua` on stdin, never on its command line. The
/// Electron host answers the same methods (`src/bridge/ci.ts`).
enum CiRunners {
    /// The `cua ci app` operation each method runs.
    static let operations: [String: String] = [
        "ci.overview": "overview",
        "ci.connect": "connect",
        "ci.connectToken": "connect_token",
        "ci.connectCancel": "connect_cancel",
        "ci.repositories": "repositories",
        "ci.goal": "goal",
        "ci.log": "log",
    ]

    /// Under the page's 30 second request limit: a slow service answers with
    /// a failure the page can show.
    static let timeout: TimeInterval = 25

    private static func string(_ v: Any?) -> String? {
        guard let s = v as? String, !s.trimmingCharacters(in: .whitespaces).isEmpty else { return nil }
        return s
    }

    private static func strings(_ v: Any?) -> [String] {
        (v as? [Any] ?? []).compactMap { string($0) }
    }

    /// The JSON the operation takes, from the page's arguments: only its keys, each checked.
    static func input(method: String, args: [String: Any]) throws -> [String: Any] {
        switch method {
        case "ci.overview":
            return ["live": (args["live"] as? Bool) == true]
        case "ci.connect":
            if let org = string(args["githubOrg"]) { return ["githubOrg": org] }
            return [:]
        case "ci.connectToken":
            guard let org = string(args["org"]) else { throw WebUIBridge.Failure.badArgs("org: string") }
            if let token = string(args["token"]) { return ["org": org, "token": token] }
            return ["org": org]
        case "ci.connectCancel":
            return [:]
        case "ci.repositories":
            return ["allow": strings(args["allow"]), "remove": strings(args["remove"])]
        case "ci.goal":
            guard let target = string(args["target"]), let goal = string(args["goal"]) else {
                throw WebUIBridge.Failure.badArgs("target and goal: string")
            }
            return ["target": target, "goal": goal]
        case "ci.log":
            guard let key = string(args["jobKey"]), let pool = string(args["pool"]) else {
                throw WebUIBridge.Failure.badArgs("jobKey and pool: string")
            }
            return ["jobKey": key, "pool": pool, "slot": string(args["slot"]) ?? ""]
        default:
            throw WebUIBridge.Failure.unimplemented(method)
        }
    }

    /// The arguments after `cua`.
    static func arguments(operation: String) -> [String] { ["ci", "app", operation] }

    /// What `cua ci app` printed (its last JSON line): the result, or the
    /// failure with its code mapped to the bridge's.
    static func answer(stdout: String, stderr: String, status: Int32) throws -> Any {
        if status != 0 {
            let last = stderr.split(whereSeparator: \.isNewline)
                .map { $0.trimmingCharacters(in: .whitespaces) }
                .last { !$0.isEmpty }
            throw WebUIBridge.Failure(code: "failed", message: last ?? "cua ci app exited with status \(status)")
        }
        let line = stdout.split(whereSeparator: \.isNewline)
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .last { !$0.isEmpty }
        guard let data = line?.data(using: .utf8),
              let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] else {
            throw WebUIBridge.Failure(code: "failed", message: "cua ci app answered something that is not JSON")
        }
        if object["ok"] as? Bool == true { return object["result"] ?? NSNull() }
        let error = object["error"] as? [String: Any]
        let message = error?["message"] as? String ?? "CI Runners could not do that"
        switch error?["code"] as? String {
        case "invalid": throw WebUIBridge.Failure.badArgs(message)
        case "not_found": throw WebUIBridge.Failure.notFound(message)
        case "unsupported": throw WebUIBridge.Failure.unsupported(message)
        default: throw WebUIBridge.Failure(code: "failed", message: message)
        }
    }

    /// Runs `cua ci app <operation>` off the main thread with `input` on
    /// stdin; stopped after `timeout` seconds.
    static func run(cua: String, operation: String, input: [String: Any], timeout: TimeInterval = CiRunners.timeout) async -> (status: Int32, stdout: String, stderr: String) {
        await Task.detached { () -> (Int32, String, String) in
            let process = Process()
            process.executableURL = URL(fileURLWithPath: cua)
            process.arguments = arguments(operation: operation)
            process.environment = ProcessInfo.processInfo.environment
            let stdin = Pipe(), stdout = Pipe(), stderr = Pipe()
            process.standardInput = stdin
            process.standardOutput = stdout
            process.standardError = stderr
            do { try process.run() } catch { return (-1, "", "Could not start cua: \(error.localizedDescription)") }
            let timer = DispatchSource.makeTimerSource(queue: .global())
            nonisolated(unsafe) var timedOut = false
            timer.schedule(deadline: .now() + timeout)
            timer.setEventHandler { timedOut = true; process.terminate() }
            timer.resume()
            // The arguments, then end of input; a closed pipe (cua exited early) is cua's own failure to report.
            let body = (try? JSONSerialization.data(withJSONObject: input)) ?? Data("{}".utf8)
            try? stdin.fileHandleForWriting.write(contentsOf: body)
            try? stdin.fileHandleForWriting.close()
            // Both pipes to EOF, so a chatty child never blocks on a full one.
            nonisolated(unsafe) var errData = Data()
            let errReader = Thread { errData = stderr.fileHandleForReading.readDataToEndOfFile() }
            errReader.start()
            let outData = stdout.fileHandleForReading.readDataToEndOfFile()
            process.waitUntilExit()
            while !errReader.isFinished { usleep(1_000) }
            timer.cancel()
            if timedOut { return (-1, "", "CI Runners timed out after \(Int(timeout)) seconds") }
            return (process.terminationStatus, String(decoding: outData, as: UTF8.self), String(decoding: errData, as: UTF8.self))
        }.value
    }
}
