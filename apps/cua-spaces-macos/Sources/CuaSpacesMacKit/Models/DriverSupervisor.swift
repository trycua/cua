// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Darwin
import Foundation

/// What Cua Driver's login item last reported, in
/// [`EmbeddedDriver.statusFile`] (`<CuaHome>/run/cua-driver.status.json`).
/// The Cua Driver page reads it to say why the driver isn't running.
public struct DriverSupervisorReport: Codable, Equatable, Sendable {
    /// `starting`, `ready`, `paused`, `waiting` (to start again after a
    /// failure) or `stopped`.
    public var state: String
    /// The running driver's pid (`ready`).
    public var pid: Int32?
    /// Why the last start failed or the driver stopped, in plain words.
    public var lastError: String?
    /// Restarts since the login item started.
    public var restarts: Int
    /// The login item's own pid.
    public var supervisorPid: Int32
    /// Seconds since 1970.
    public var at: Double

    public static func read(_ path: String) -> DriverSupervisorReport? {
        guard let data = FileManager.default.contents(atPath: path) else { return nil }
        return try? JSONDecoder().decode(DriverSupervisorReport.self, from: data)
    }

    /// Whether the login item that wrote this is still running.
    public var supervisorAlive: Bool { supervisorPid > 0 && (kill(supervisorPid, 0) == 0 || errno == EPERM) }
}

/// Keeps `cua-driver serve --embedded` running for the coding agents on
/// this Mac, as a direct child of the process that runs it (the app's Cua
/// Driver login item, `CuaSpacesMac --driver-helper run`), so macOS holds
/// Cua Spaces responsible for it. Starts it, waits until it answers on its
/// socket, restarts it with a backoff (1 s, doubling, at most a minute)
/// when it exits, stops it while the person has paused agents, and writes
/// every step and the last error to [`EmbeddedDriver.statusFile`].
///
/// The driver is started with a stdin pipe it watches
/// (`--parent-liveness-stdio`): when this process goes, so does it.
public final class DriverSupervisor: @unchecked Sendable {
    public struct Options: Sendable {
        /// How long a starting driver has to answer on its socket.
        public var startupTimeout: TimeInterval = 20
        /// Uptime after which an exit is a fresh failure, not a crash loop.
        public var stableUptime: TimeInterval = 120
        /// How often the loop looks at the child, the pause marker and the
        /// start request.
        public var poll: TimeInterval = 0.25
        /// The longest wait between two starts.
        public var maxDelay: TimeInterval = 60
        public init() {}
    }

    public let driver: EmbeddedDriver
    let options: Options
    private var child: Process?
    private var liveness: Pipe?
    private var restarts = 0
    private var lastError: String?

    public init(driver: EmbeddedDriver, options: Options = Options()) {
        self.driver = driver
        self.options = options
    }

    /// The wait before start number `attempt + 1` after `attempt` failures
    /// in a row: 1 s, doubling, at most `max`.
    public static func restartDelay(attempt: Int, max: TimeInterval = 60) -> TimeInterval {
        min(max, pow(2, Double(Swift.max(0, Swift.min(attempt, 16) - 1))))
    }

    /// `cua-driver serve --embedded ...`: what the driver SDK's embedded
    /// host passes (`EmbeddedCuaDriverHost::serve_args`).
    public static func serveArguments(_ d: EmbeddedDriver) -> [String] {
        ["serve", "--embedded", "--parent-liveness-stdio", "--no-permissions-gate",
         "--socket", d.socket, "--host-bundle-id", d.hostBundleId, "--permission-mode", "standard"]
    }

    /// The driver's environment: only what the SDK's embedded host passes
    /// on (`allowed_environment_name`), plus who hosts it.
    public static func childEnvironment(_ inherited: [String: String], hostPid: Int32) -> [String: String] {
        let names: Set<String> = ["PATH", "HOME", "USER", "LOGNAME", "SHELL", "TMPDIR", "TMP", "TEMP", "LANG"]
        var env = inherited.filter { names.contains($0.key) || $0.key.hasPrefix("LC_") || $0.key.hasPrefix("CUA_DRIVER_") }
        env["CUA_DRIVER_EMBEDDED_HOST_PID"] = String(hostPid)
        return env
    }

    // MARK: - The loop

    /// Runs until `shouldStop` says so, then stops the driver. Never
    /// returns early: every failure is reported and retried.
    public func run(shouldStop: () -> Bool) {
        var attempt = 0
        outer: while !shouldStop() {
            if let error = prepareRunDirectory() {
                lastError = error
                report("waiting")
                if sleepWatching(5, shouldStop: shouldStop) == .stop { break }
                continue
            }
            if paused {
                report("paused")
                attempt = 0
                if sleepWatching(options.poll, shouldStop: shouldStop) == .stop { break }
                continue
            }
            // A request written while nothing waited for it is answered by
            // this start.
            _ = takeStartRequest()
            report("starting")
            let started = Date()
            switch launch(shouldStop: shouldStop) {
            case .stopped:
                break outer
            case let .failed(reason):
                lastError = reason
                NSLog("cua-spaces driver: Cua Driver didn't start: %@", reason)
            case let .ready(pid):
                lastError = nil
                report("ready", pid: pid)
                NSLog("cua-spaces driver: Cua Driver is running (pid %d)", pid)
                watch: while true {
                    if shouldStop() { break outer }
                    if paused {
                        NSLog("cua-spaces driver: agents paused; stopping Cua Driver")
                        stopChild()
                        attempt = 0
                        continue outer
                    }
                    if let child, !child.isRunning {
                        lastError = "Cua Driver stopped (\(Self.exitText(child)))\(tail(of: child))"
                        NSLog("cua-spaces driver: %@", lastError ?? "")
                        self.child = nil
                        liveness = nil
                        break watch
                    }
                    Thread.sleep(forTimeInterval: options.poll)
                }
            }
            if Date().timeIntervalSince(started) >= options.stableUptime { attempt = 0 }
            attempt += 1
            restarts += 1
            report("waiting")
            if sleepWatching(Self.restartDelay(attempt: attempt, max: options.maxDelay), shouldStop: shouldStop,
                             wakeOnStartRequest: true) == .stop { break }
        }
        stopChild()
        report("stopped")
    }

    enum Launch: Equatable {
        case ready(Int32)
        case failed(String)
        case stopped
    }

    private var logOffset: UInt64 = 0

    /// Starts the driver and waits until it answers on its socket as this
    /// child, embedded, for this app.
    func launch(shouldStop: () -> Bool) -> Launch {
        guard FileManager.default.isExecutableFile(atPath: driver.binary) else {
            return .failed("there is no Cua Driver at \(driver.binary); reinstall Cua Spaces")
        }
        if let conflict = Self.prepareEndpoint(driver.socket) { return .failed(conflict) }
        let log = openLog()
        let p = Process()
        p.executableURL = URL(fileURLWithPath: driver.binary)
        p.arguments = Self.serveArguments(driver)
        p.environment = Self.childEnvironment(ProcessInfo.processInfo.environment, hostPid: getpid())
        let pipe = Pipe()
        p.standardInput = pipe
        p.standardOutput = FileHandle.nullDevice
        p.standardError = log ?? FileHandle.nullDevice
        do {
            try p.run()
        } catch {
            return .failed("couldn't run \(driver.binary): \(error.localizedDescription)")
        }
        try? log?.close()
        child = p
        liveness = pipe
        let deadline = Date().addingTimeInterval(options.startupTimeout)
        while true {
            if shouldStop() {
                stopChild()
                return .stopped
            }
            if !p.isRunning {
                child = nil
                liveness = nil
                return .failed("Cua Driver quit while starting (\(Self.exitText(p)))\(tail(of: p))")
            }
            if let meta = driver.metadata(timeout: 1), meta.pid == p.processIdentifier {
                guard meta.embedded, meta.hostBundleId == driver.hostBundleId else {
                    stopChild()
                    return .failed("Cua Driver started, but not as part of Cua Spaces (embedded: \(meta.embedded), host: \(meta.hostBundleId ?? "none"))")
                }
                return .ready(p.processIdentifier)
            }
            if Date() >= deadline {
                stopChild()
                return .failed("Cua Driver didn't answer within \(Int(options.startupTimeout)) s\(tail(of: nil))")
            }
            Thread.sleep(forTimeInterval: 0.1)
        }
    }

    /// Stops the running driver: closes its stdin (it exits on its own),
    /// then SIGTERM, then SIGKILL.
    func stopChild() {
        guard let p = child else { return }
        child = nil
        try? liveness?.fileHandleForWriting.close()
        liveness = nil
        if waitExit(p, seconds: 5) { return }
        p.terminate()
        if waitExit(p, seconds: 2) { return }
        kill(p.processIdentifier, SIGKILL)
        _ = waitExit(p, seconds: 2)
    }

    private func waitExit(_ p: Process, seconds: TimeInterval) -> Bool {
        let end = Date().addingTimeInterval(seconds)
        while p.isRunning, Date() < end { Thread.sleep(forTimeInterval: 0.05) }
        return !p.isRunning
    }

    // MARK: - Files

    /// Clears the socket path before a start, as the SDK's embedded host
    /// does (`prepare_endpoint`): a driver that died (a crash, SIGKILL)
    /// leaves its socket file, and an embedded driver refuses to start over
    /// one. A socket nobody accepts on is removed; one that answers, or a
    /// path that isn't a socket, is reported. Nil, or why not.
    public static func prepareEndpoint(_ path: String) -> String? {
        var before = stat()
        guard lstat(path, &before) == 0 else {
            return errno == ENOENT ? nil : "can't read \(path): \(String(cString: strerror(errno)))"
        }
        guard before.st_mode & S_IFMT == S_IFSOCK else {
            return "\(path) is in the way (not a socket); remove it"
        }
        if acceptsConnections(path) {
            return "another Cua Driver is already running on \(path)"
        }
        var now = stat()
        guard lstat(path, &now) == 0, now.st_mode & S_IFMT == S_IFSOCK,
              now.st_dev == before.st_dev, now.st_ino == before.st_ino else {
            return "\(path) changed while it was checked"
        }
        guard unlink(path) == 0 else {
            return "can't remove the old socket \(path): \(String(cString: strerror(errno)))"
        }
        return nil
    }

    static func acceptsConnections(_ path: String) -> Bool {
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { return false }
        defer { close(fd) }
        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(path.utf8)
        guard bytes.count < MemoryLayout.size(ofValue: addr.sun_path) else { return false }
        withUnsafeMutableBytes(of: &addr.sun_path) { raw in
            raw.copyBytes(from: bytes)
            raw[bytes.count] = 0
        }
        return withUnsafePointer(to: &addr) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) }
        } == 0
    }

    var paused: Bool { FileManager.default.fileExists(atPath: driver.pausedMarker) }

    func takeStartRequest() -> Bool {
        (try? FileManager.default.removeItem(atPath: driver.startRequest)) != nil
    }

    /// `<CuaHome>/run`, 0700 (the socket lives there). Nil, or why not.
    func prepareRunDirectory() -> String? {
        let dir = (driver.socket as NSString).deletingLastPathComponent
        do {
            try FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
            try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: dir)
            return nil
        } catch {
            return "can't create \(dir): \(error.localizedDescription)"
        }
    }

    /// The driver's log, opened for appending; trimmed when over 8 MB.
    private func openLog() -> FileHandle? {
        let path = driver.logFile
        try? FileManager.default.createDirectory(atPath: (path as NSString).deletingLastPathComponent, withIntermediateDirectories: true)
        if let size = (try? FileManager.default.attributesOfItem(atPath: path))?[.size] as? UInt64, size > 8 << 20 {
            try? FileManager.default.removeItem(atPath: path)
        }
        if !FileManager.default.fileExists(atPath: path) {
            FileManager.default.createFile(atPath: path, contents: nil, attributes: [.posixPermissions: 0o600])
        }
        guard let h = FileHandle(forWritingAtPath: path) else { return nil }
        logOffset = (try? h.seekToEnd()) ?? 0
        return h
    }

    /// ": <the driver's last line of output since it started>", or "".
    private func tail(of _: Process?) -> String {
        guard let h = FileHandle(forReadingAtPath: driver.logFile) else { return "" }
        defer { try? h.close() }
        let end = (try? h.seekToEnd()) ?? 0
        let from = max(logOffset, end > 65536 ? end - 65536 : 0)
        guard end > from, (try? h.seek(toOffset: from)) != nil, let data = try? h.readToEnd() else { return "" }
        let line = String(decoding: data, as: UTF8.self)
            .split(whereSeparator: \.isNewline)
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .last { !$0.isEmpty } ?? ""
        return line.isEmpty ? "" : ": " + String(line.prefix(300))
    }

    static func exitText(_ p: Process) -> String {
        p.terminationReason == .uncaughtSignal ? "signal \(p.terminationStatus)" : "exit code \(p.terminationStatus)"
    }

    private func report(_ state: String, pid: Int32? = nil) {
        let r = DriverSupervisorReport(state: state, pid: pid, lastError: lastError, restarts: restarts,
                                       supervisorPid: getpid(), at: Date().timeIntervalSince1970)
        guard let data = try? JSONEncoder().encode(r) else { return }
        try? data.write(to: URL(fileURLWithPath: driver.statusFile), options: .atomic)
    }

    private enum Wake { case stop, time, request, pause }

    /// Sleeps `seconds`, or less when asked to stop, when the person pauses
    /// agents, or (`wakeOnStartRequest`) when the app asks for a start now.
    private func sleepWatching(_ seconds: TimeInterval, shouldStop: () -> Bool, wakeOnStartRequest: Bool = false) -> Wake {
        let end = Date().addingTimeInterval(seconds)
        let wasPaused = paused
        while Date() < end {
            if shouldStop() { return .stop }
            if wakeOnStartRequest, takeStartRequest() {
                NSLog("cua-spaces driver: Cua Spaces asked for Cua Driver now; starting it")
                return .request
            }
            if paused != wasPaused { return .pause }
            Thread.sleep(forTimeInterval: min(options.poll, max(0, end.timeIntervalSinceNow)))
        }
        return .time
    }
}
