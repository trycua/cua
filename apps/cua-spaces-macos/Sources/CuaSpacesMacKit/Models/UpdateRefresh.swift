// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
import Darwin
import Foundation

/// A command's result.
public struct CommandResult: Sendable, Equatable {
    public var status: Int32
    public var output: String
    public var timedOut: Bool

    public init(status: Int32, output: String, timedOut: Bool = false) {
        self.status = status
        self.output = output
        self.timedOut = timedOut
    }
}

/// Runs the bundled `cua` (a fake in tests).
public protocol CommandRunning: Sendable {
    func run(_ executable: String, _ arguments: [String], timeout: TimeInterval) -> CommandResult
}

/// `Process`, bounded by `timeout` (then terminated). Standard output is
/// kept (bounded); standard error is dropped.
public struct ProcessRunner: CommandRunning {
    public init() {}

    public func run(_ executable: String, _ arguments: [String], timeout: TimeInterval) -> CommandResult {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: executable)
        process.arguments = arguments
        process.standardInput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice
        let pipe = Pipe()
        process.standardOutput = pipe
        let done = DispatchSemaphore(value: 0)
        process.terminationHandler = { _ in done.signal() }
        do { try process.run() } catch {
            return CommandResult(status: -1, output: error.localizedDescription)
        }
        // Read while it runs so a full pipe never blocks it.
        let reader = OutputReader(pipe.fileHandleForReading)
        var timedOut = false
        if done.wait(timeout: .now() + timeout) == .timedOut {
            timedOut = true
            process.terminate()
            if done.wait(timeout: .now() + 2) == .timedOut { kill(process.processIdentifier, SIGKILL); done.wait() }
        }
        return CommandResult(status: process.terminationStatus, output: reader.finish(), timedOut: timedOut)
    }
}

/// Collects a pipe's output on a background queue (at most 1 MiB).
private final class OutputReader: @unchecked Sendable {
    private var data = Data()
    private let lock = NSLock()
    private let finished = DispatchSemaphore(value: 0)

    init(_ handle: FileHandle) {
        DispatchQueue.global().async { [self] in
            while true {
                let chunk = handle.availableData
                if chunk.isEmpty { break }
                lock.lock()
                if data.count < 1 << 20 { data.append(chunk) }
                lock.unlock()
            }
            finished.signal()
        }
    }

    func finish() -> String {
        _ = finished.wait(timeout: .now() + 2)
        lock.lock()
        defer { lock.unlock() }
        return String(decoding: data, as: UTF8.self)
    }
}

/// The first launch after an update: refresh what cua installed into the
/// coding agents (`cua agents update`, which skips folders the user edited),
/// with the bundled `cua`. This app's own daemon (`appAboutRestartDaemon`
/// says which is) is replaced by `DaemonSupervisor`'s `cua daemon start`,
/// which knows a daemon whose executable was updated since it started. The
/// core decides (`appAboutAfterLaunch`, `appAboutRefreshNotice`); this runs
/// the commands.
public struct UpdateRefresh: Sendable {
    /// The bundled `cua`.
    public var cua: String
    /// This app's bundle.
    public var bundle: String
    public var runner: CommandRunning

    public init(cua: String, bundle: String, runner: CommandRunning = ProcessRunner()) {
        self.cua = cua
        self.bundle = bundle
        self.runner = runner
    }

    /// Records this launch's version in the settings file and says whether
    /// it follows an update (a fresh install and a relaunch do not).
    @discardableResult
    public static func record(settingsPath: String, version: String, build: String,
                              onboarded: Bool) -> Bool {
        var settings = appSettingsLoad(path: settingsPath)
        let plan = appAboutAfterLaunch(input: AppLaunchInput(
            lastSeen: settings.lastSeenVersion, version: version, build: build, onboarded: onboarded))
        if let save = plan.save {
            settings.lastSeenVersion = save
            try? appSettingsSave(path: settingsPath, settings: settings)
        }
        return plan.refresh
    }

    /// `cua agents update`. Returns what failed, or nil.
    public func updateAgents() -> String? {
        let r = runner.run(cua, ["--json", "agents", "update"], timeout: 120)
        if r.timedOut { return "`cua agents update` did not finish within 2 minutes" }
        if r.status == 0 { return nil }
        return Self.failures(r.output) ?? "`cua agents update` exited with status \(r.status)"
    }

    /// The agents' refresh; the notice to show, or nil when it worked.
    /// (This app's daemon is replaced at launch by `cua daemon start`,
    /// `DaemonSupervisor`, when its executable was updated.)
    public func run() -> String? {
        appAboutRefreshNotice(report: AppRefreshReport(agentsError: updateAgents(), daemonError: nil))
    }

    /// The JSON object `cua --json` printed (pretty-printed, maybe after
    /// other lines).
    static func json(_ output: String) -> [String: Any]? {
        let trimmed = output.trimmingCharacters(in: .whitespacesAndNewlines)
        if let obj = try? JSONSerialization.jsonObject(with: Data(trimmed.utf8)) as? [String: Any] { return obj }
        guard let start = trimmed.range(of: "\n{", options: .backwards) else { return nil }
        return try? JSONSerialization.jsonObject(with: Data(trimmed[start.lowerBound...].utf8)) as? [String: Any]
    }

    /// The failed outcomes of `cua --json agents update` ("skill cua: why").
    static func failures(_ output: String) -> String? {
        guard let outcomes = json(output)?["outcomes"] as? [[String: Any]] else { return nil }
        let failed = outcomes.filter { $0["change"] as? String == "failed" }.map { o in
            let item = o["item"] as? String ?? "?"
            let detail = o["detail"] as? String ?? ""
            return detail.isEmpty ? item : "\(item): \(detail)"
        }
        return failed.isEmpty ? nil : failed.joined(separator: "; ")
    }

    /// A process's executable: `proc_pidpath`, else (its file is gone: an
    /// update replaced the bundle it ran from, so it is an older build) the
    /// path it was started from (`KERN_PROCARGS2`).
    public static func executablePath(_ pid: Int32) -> String? {
        guard pid > 0 else { return nil }
        var buffer = [CChar](repeating: 0, count: 4 * Int(MAXPATHLEN))
        let n = proc_pidpath(pid, &buffer, UInt32(buffer.count))
        if n > 0 { return String(decoding: buffer.prefix(Int(n)).map { UInt8(bitPattern: $0) }, as: UTF8.self) }
        return launchPath(pid)
    }

    /// The executable path a process was started with (`KERN_PROCARGS2`:
    /// argc, then that path).
    static func launchPath(_ pid: Int32) -> String? {
        var mib: [Int32] = [CTL_KERN, KERN_PROCARGS2, pid]
        var size = 0
        guard sysctl(&mib, 3, nil, &size, nil, 0) == 0, size > MemoryLayout<Int32>.size else { return nil }
        var bytes = [UInt8](repeating: 0, count: size)
        guard sysctl(&mib, 3, &bytes, &size, nil, 0) == 0 else { return nil }
        let path = bytes[MemoryLayout<Int32>.size..<size].prefix { $0 != 0 }
        return path.isEmpty ? nil : String(decoding: path, as: UTF8.self)
    }
}
