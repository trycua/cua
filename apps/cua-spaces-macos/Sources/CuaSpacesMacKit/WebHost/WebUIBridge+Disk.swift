// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import Foundation

/// What `disk.get` and `disk.do` run: the bundled `cua storage` (the same
/// code the CLI runs) and, for Uninstall, the bundled `uninstall.sh`.
/// The pure parts (the words the page reads, the arguments) are here so
/// tests need no process.
enum DiskStorage {
    /// `cua --json storage show` as the page reads it (camelCase).
    static func view(from json: [String: Any], canUninstall: Bool) -> [String: Any] {
        func num(_ v: Any?) -> Int { (v as? NSNumber)?.intValue ?? 0 }
        let kinds = (json["kinds"] as? [[String: Any]] ?? []).map { k -> [String: Any] in
            ["kind": k["kind"] as? String ?? "", "path": k["path"] as? String ?? "",
             "source": k["source"] as? String ?? "default", "movable": k["movable"] as? Bool ?? false,
             "bytes": num(k["bytes"])]
        }
        let items = (json["items"] as? [[String: Any]] ?? []).map { i -> [String: Any] in
            ["family": i["family"] as? String ?? "", "name": i["name"] as? String ?? "",
             "path": i["path"] as? String ?? "", "bytes": num(i["bytes"])]
        }
        let old = (json["old_locations"] as? [[String: Any]] ?? []).map { o -> [String: Any] in
            ["path": o["path"] as? String ?? "", "bytes": num(o["bytes"])]
        }
        let caps = json["caps"] as? [String: Any] ?? [:]
        return [
            "home": json["home"] as? String ?? "", "homeBytes": num(json["home_bytes"]),
            "freeBytes": (json["free_bytes"] as? NSNumber).map { $0.intValue as Any } ?? NSNull(),
            "daemonRunning": json["daemon_running"] as? Bool ?? false,
            "kinds": kinds, "items": items, "oldLocations": old,
            "trajectoriesMaxBytes": num(caps["trajectories_max_bytes"]),
            "trajectoriesMaxDays": num(caps["trajectories_max_days"]),
            "canUninstall": canUninstall,
        ]
    }

    /// The `cua` arguments for a command; nil for the ones that are not a
    /// `cua` call (`locate`, `uninstall`).
    static func arguments(for command: [String: Any]) throws -> [String]? {
        func s(_ k: String) throws -> String {
            guard let v = command[k] as? String, !v.isEmpty else { throw WebUIBridge.Failure.badArgs("\(k): string") }
            return v
        }
        let movable = ["runtimes", "images", "cache", "logs", "trajectories"]
        func kind() throws -> String {
            let k = try s("kind")
            guard movable.contains(k) else { throw WebUIBridge.Failure.badArgs("kind: one of \(movable.joined(separator: ", "))") }
            return k
        }
        func path() throws -> String {
            let p = try s("path")
            guard p.hasPrefix("/") else { throw WebUIBridge.Failure.badArgs("path: a full path") }
            return p
        }
        switch command["op"] as? String ?? "" {
        case "check": return ["--json", "storage", "check", try kind(), try path()]
        case "set":
            var a = ["--json", "storage", "set", try kind(), try path()]
            if command["move"] as? Bool == true { a.append("--move") }
            return a
        case "reset": return ["--json", "storage", "reset", try kind()]
        case "remove": return ["--json", "storage", "remove", try path()]
        case "locate", "uninstall": return nil
        default: throw WebUIBridge.Failure.badArgs("op")
        }
    }

    /// The answer the page reads, from `cua`'s exit status and output.
    static func result(status: Int32, output: String, timedOut: Bool = false) -> [String: Any] {
        var out: [String: Any] = ["ok": status == 0, "path": NSNull(), "error": NSNull(), "warnings": [String](),
                                  "neededBytes": NSNull(), "availableBytes": NSNull(), "freedBytes": NSNull()]
        let json = output.data(using: .utf8).flatMap { try? JSONSerialization.jsonObject(with: $0) } as? [String: Any]
        if let json {
            if let ok = json["ok"] as? Bool { out["ok"] = ok && status == 0 }
            if let e = json["error"] as? String { out["error"] = e }
            if let w = json["warnings"] as? [String] { out["warnings"] = w }
            if let n = json["needed_bytes"] as? NSNumber { out["neededBytes"] = n }
            if let a = json["available_bytes"] as? NSNumber { out["availableBytes"] = a }
            if let f = json["freed_bytes"] as? NSNumber { out["freedBytes"] = f }
        }
        if out["ok"] as? Bool == false, out["error"] is NSNull {
            let text = output.trimmingCharacters(in: .whitespacesAndNewlines)
            out["error"] = timedOut ? "That took too long." : (text.isEmpty ? "It did not work." : String(text.suffix(400)))
        }
        return out
    }

    /// Arguments of the uninstall script: everything but the Keychain unless asked.
    static func uninstallArguments(keychain: Bool, appPid: Int32) -> [String] {
        var a = ["--yes", "--wait-for-pid", String(appPid)]
        if !keychain { a.append("--keep-keychain") }
        return a
    }

    /// Runs `executable` with stdout and stderr together; returns the status and output.
    static func run(_ executable: String, _ arguments: [String], timeout: TimeInterval) -> (status: Int32, output: String, timedOut: Bool) {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: executable)
        process.arguments = arguments
        process.standardInput = FileHandle.nullDevice
        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = pipe
        let done = DispatchSemaphore(value: 0)
        process.terminationHandler = { _ in done.signal() }
        do { try process.run() } catch { return (-1, error.localizedDescription, false) }
        let collected = Collected()
        let reader = DispatchQueue(label: "disk-storage-output")
        let finished = DispatchSemaphore(value: 0)
        reader.async {
            while true {
                let chunk = pipe.fileHandleForReading.availableData
                if chunk.isEmpty { break }
                collected.append(chunk)
            }
            finished.signal()
        }
        var timedOut = false
        if done.wait(timeout: .now() + timeout) == .timedOut {
            timedOut = true
            process.terminate()
            if done.wait(timeout: .now() + 2) == .timedOut { kill(process.processIdentifier, SIGKILL); done.wait() }
        }
        _ = finished.wait(timeout: .now() + 2)
        return (process.terminationStatus, collected.text, timedOut)
    }

    private final class Collected: @unchecked Sendable {
        private var data = Data()
        private let lock = NSLock()
        func append(_ d: Data) { lock.lock(); if data.count < 1 << 20 { data.append(d) }; lock.unlock() }
        var text: String { lock.lock(); defer { lock.unlock() }; return String(decoding: data, as: UTF8.self) }
    }
}

extension WebUIBridge {
    /// Where the app's uninstall script is (`Contents/Resources/uninstall.sh`).
    static var bundledUninstallScript: String? {
        Bundle.main.resourcePath.map { $0 + "/uninstall.sh" }.flatMap {
            FileManager.default.isReadableFile(atPath: $0) ? $0 : nil
        }
    }

    func disk(_ method: String, _ args: [String: Any]) async throws -> Any {
        guard let cua = AppEnvironment.bundledCuaPath else {
            throw Failure.unsupported("Storage settings need the app's own cua")
        }
        if method == "disk.get" {
            let r = await Task.detached { DiskStorage.run(cua, ["--json", "storage", "show"], timeout: 60) }.value
            guard r.status == 0, let json = r.output.data(using: .utf8).flatMap({ Self.lastJSONObject($0) }) else {
                throw Failure(code: "failed", message: "Cua could not read its folders")
            }
            return DiskStorage.view(from: json, canUninstall: Self.bundledUninstallScript != nil)
        }
        switch args["op"] as? String ?? "" {
        case "locate":
            let kind = try string(args, "kind")
            let panel = NSOpenPanel()
            panel.canChooseDirectories = true
            panel.canChooseFiles = false
            panel.canCreateDirectories = true
            panel.allowsMultipleSelection = false
            panel.message = "Choose a folder for this. Cua copies the files there."
            panel.prompt = "Choose"
            _ = kind
            var r = DiskStorage.result(status: 0, output: "")
            if panel.runModal() == .OK, let url = panel.url { r["path"] = url.path }
            return r
        case "uninstall":
            return try startUninstall(keychain: args["keychain"] as? Bool == true)
        default:
            guard let arguments = try DiskStorage.arguments(for: args) else { throw Failure.badArgs("op") }
            let r = await Task.detached { DiskStorage.run(cua, arguments, timeout: 6 * 3600) }.value
            return DiskStorage.result(status: r.status, output: r.output, timedOut: r.timedOut)
        }
    }

    /// The first JSON object in `data` (the CLI may print a notice first).
    static func lastJSONObject(_ data: Data) -> [String: Any]? {
        if let o = try? JSONSerialization.jsonObject(with: data) as? [String: Any] { return o }
        let text = String(decoding: data, as: UTF8.self)
        guard let start = text.firstIndex(of: "{") else { return nil }
        return (try? JSONSerialization.jsonObject(with: Data(text[start...].utf8))) as? [String: Any]
    }

    /// Copies the script out of the app, starts it detached, then quits: the
    /// script waits for this process to exit before it removes the app.
    func startUninstall(keychain: Bool) throws -> Any {
        guard let script = Self.bundledUninstallScript else {
            throw Failure.unsupported("This build cannot uninstall itself")
        }
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("cua-uninstall-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let copy = dir.appendingPathComponent("uninstall.sh")
        try FileManager.default.copyItem(atPath: script, toPath: copy.path)
        let args = DiskStorage.uninstallArguments(keychain: keychain, appPid: ProcessInfo.processInfo.processIdentifier)
        let log = dir.appendingPathComponent("uninstall.log").path
        let quoted = ([copy.path] + args).map { "'" + $0.replacingOccurrences(of: "'", with: "'\\''") + "'" }.joined(separator: " ")
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/bin/sh")
        p.arguments = ["-c", "nohup /bin/sh \(quoted) >'\(log)' 2>&1 &"]
        p.standardInput = FileHandle.nullDevice
        try p.run()
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.6) { NSApp.terminate(nil) }
        return DiskStorage.result(status: 0, output: "")
    }
}
