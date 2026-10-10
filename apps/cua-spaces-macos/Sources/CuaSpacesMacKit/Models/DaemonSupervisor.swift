// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpacesFFI
import Foundation

/// This app's own `cua daemon`: the bundled `cua`'s build. The Keyvault, the
/// persistent-agent supervisor, Cua Volume and host Spaces live only in the
/// daemon, so the app starts it at launch, has it replace a daemon of
/// another build (another app's older one, or its own from before an
/// update; another app's of the same or a newer version is used), and
/// starts it again when it dies. `cua daemon start` does the work: it
/// starts a daemon that survives the app (as the Tauri app's does), replaces
/// a stranger, and does nothing when this build's already runs.
///
/// Another app's daemon is replaced at most once per session: an older app
/// (an installed release) starts its own again, and replacing it each time
/// would only take turns with it. Its executable then goes in
/// `CUA_DAEMON_KEEP`, so `cua daemon start` and the SDK keep and use that
/// daemon when it comes back, and the app says so (`yieldNotice`).
public final class DaemonSupervisor: @unchecked Sendable {
    /// `CUA_DAEMON_KEEP` (cua-daemon's `identity::KEEP_ENV`): other apps'
    /// daemon executables this app replaced once.
    public static let keepEnv = "CUA_DAEMON_KEEP"

    /// The bundled `cua`.
    public let cua: String
    private let lock = NSLock()
    /// Other apps' daemon executables (symlinks resolved) this app
    /// replaced, once each.
    private var replaced: [String] = []
    /// Sets the keep list where `cua daemon start` and the SDK read it
    /// (this process's environment; tests replace it).
    var publishKeep: (String) -> Void = { setenv(DaemonSupervisor.keepEnv, $0, 1) }
    /// This app's bundle (`<App>.app`, holding `Contents/MacOS/cua`).
    public let bundle: String

    /// `nil` when the bundle has no executable `cua` (a bare build).
    public init?(bundledCua: String) {
        guard FileManager.default.isExecutableFile(atPath: bundledCua) else { return nil }
        cua = bundledCua
        // <App>.app/Contents/MacOS/cua -> <App>.app
        bundle = URL(fileURLWithPath: bundledCua).deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent().path
    }

    /// Whether the daemon `pid` is this app's own: its executable is in
    /// this bundle (the core's rule, `appAboutRestartDaemon`, which the
    /// refresh after an update used too). Which build it runs is the
    /// daemon identity's business: `cua daemon start` replaces one from
    /// before an update.
    public func isOwn(pid: UInt32) -> Bool {
        appAboutRestartDaemon(check: AppDaemonCheck(
            daemonExe: UpdateRefresh.executablePath(Int32(bitPattern: pid)), bundle: bundle))
    }

    /// Runs `cua daemon start`, bounded by `timeout`: `nil` when this app's
    /// daemon runs afterwards, else why not. Replacing a daemon of another
    /// build waits up to 30 s for it to stop (one with Spaces running takes
    /// that long), then up to 10 s for this build's to start.
    ///
    /// Another app's daemon it replaced is remembered, so that daemon is kept
    /// when its app starts it again (`CUA_DAEMON_KEEP`).
    public func start(timeout: TimeInterval = 60) -> String? {
        let before = stranger()
        let error = run(["daemon", "start"], timeout: timeout)
        // Replaced: another daemon is named now, or that one is gone.
        if error == nil, let before,
           Self.runningPid(cuaHome: cuaHome) != before.pid || (kill(before.pid, 0) != 0 && errno == ESRCH) {
            let list: String? = lock.withLock {
                guard !replaced.contains(before.executable) else { return nil }
                replaced.append(before.executable)
                return replaced.joined(separator: ":")
            }
            if let list {
                publishKeep(list)
                NSLog("Cua Spaces: replaced another app's cua daemon (%@); if it comes back, it is kept", before.executable)
            }
        }
        return error
    }

    /// The running daemon (`daemon.json`) when it is another app's: its pid
    /// and executable (symlinks resolved).
    func stranger() -> (pid: Int32, executable: String)? {
        guard let pid = Self.runningPid(cuaHome: cuaHome), kill(pid, 0) == 0 || errno == EPERM,
              let exe = executablePath(pid), !isOwn(pid: UInt32(pid)) else { return nil }
        return (pid, Self.resolved(exe))
    }

    /// The executable of the daemon `pid` when it is another app's that this
    /// app replaced once and now yields to.
    public func yieldsTo(pid: UInt32) -> String? {
        guard let exe = executablePath(Int32(bitPattern: pid)) else { return nil }
        let real = Self.resolved(exe)
        return lock.withLock { replaced.contains(real) } ? exe : nil
    }

    /// A process's executable (tests replace it).
    var executablePath: (Int32) -> String? = { UpdateRefresh.executablePath($0) }

    static func resolved(_ path: String) -> String {
        URL(fileURLWithPath: path).resolvingSymlinksInPath().path
    }

    /// What the app says while it uses another app's daemon it yields to
    /// (that app keeps starting its own): which app, and how to get this
    /// app's daemon.
    public static func yieldNotice(executable: String, version: String?) -> String {
        let app = executable.range(of: ".app/Contents/").map { String(executable[..<$0.lowerBound]) + ".app" } ?? executable
        let which = version.map { "\(app), cua \($0)" } ?? app
        return "Another Cua Spaces app (\(which)) is running and keeps starting its own daemon, so this app uses that one. "
            + "Quit the other app, then reopen Cua Spaces to use all of this one's features."
    }

    /// Restarts this app's daemon (Try again after it did not start): asks
    /// it to stop (bounded), kills this bundle's daemon that is still
    /// starting or no longer answers (one stuck on a keychain read, say),
    /// then starts it. `nil` when it runs afterwards, else why not.
    public func restart(timeout: TimeInterval = 30) -> String? {
        _ = run(["daemon", "stop"], timeout: 15)
        for pid in Self.daemonPids(cuaHome: cuaHome) where isOwn(pid: UInt32(pid)) {
            Self.stop(pid: pid)
        }
        return start(timeout: timeout)
    }

    /// Replaces the cua home (tests).
    var homeOverride: URL?

    /// The cua home the daemon uses (`CUA_HOME`, else `~/.cua`).
    var cuaHome: URL {
        if let homeOverride { return homeOverride }
        let env = ProcessInfo.processInfo.environment
        if let home = env["CUA_HOME"], !home.isEmpty { return URL(fileURLWithPath: home, isDirectory: true) }
        return URL(fileURLWithPath: env["HOME"] ?? NSHomeDirectory(), isDirectory: true)
            .appendingPathComponent(".cua", isDirectory: true)
    }

    /// The running daemon's pid (`daemon.json`), when its file names one.
    static func runningPid(cuaHome: URL) -> Int32? {
        guard let data = try? Data(contentsOf: cuaHome.appendingPathComponent("daemon.json")),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let pid = (obj["pid"] as? NSNumber)?.int32Value, pid > 0 else { return nil }
        return pid
    }

    /// The daemon still starting (`daemon.starting`) and the one running
    /// (`daemon.json`), as their files name them.
    static func daemonPids(cuaHome: URL) -> [Int32] {
        var out: [Int32] = []
        if let text = try? String(contentsOf: cuaHome.appendingPathComponent("daemon.starting"), encoding: .utf8),
           let pid = Int32(text.trimmingCharacters(in: .whitespacesAndNewlines)) {
            out.append(pid)
        }
        if let data = try? Data(contentsOf: cuaHome.appendingPathComponent("daemon.json")),
           let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
           let pid = (obj["pid"] as? NSNumber)?.int32Value {
            out.append(pid)
        }
        return Array(Set(out.filter { $0 > 0 })).sorted()
    }

    /// SIGTERM, then SIGKILL when it is still there after `grace` s.
    static func stop(pid: Int32, grace: TimeInterval = 2) {
        guard kill(pid, SIGTERM) == 0 else { return }
        let deadline = Date().addingTimeInterval(grace)
        while kill(pid, 0) == 0, Date() < deadline { Thread.sleep(forTimeInterval: 0.05) }
        if kill(pid, 0) == 0 { kill(pid, SIGKILL) }
    }

    private func run(_ args: [String], timeout: TimeInterval) -> String? {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: cua)
        process.arguments = args
        var env = ProcessInfo.processInfo.environment
        env["CUA_DAEMON_STARTED_BY"] = "app"
        // The daemon never waits on a keychain prompt nobody may see; the
        // app asks for access itself.
        env[AppEnvironment.keychainNonInteractiveEnv] = "1"
        if let keep = lock.withLock({ replaced.isEmpty ? nil : replaced.joined(separator: ":") }) {
            env[Self.keepEnv] = keep
        }
        process.environment = env
        let output = Pipe()
        process.standardInput = FileHandle.nullDevice
        process.standardOutput = output
        process.standardError = output
        do { try process.run() } catch { return "could not run \(cua): \(error.localizedDescription)" }
        let deadline = Date().addingTimeInterval(timeout)
        while process.isRunning && Date() < deadline { Thread.sleep(forTimeInterval: 0.05) }
        if process.isRunning {
            process.terminate()
            return "`cua \(args.joined(separator: " "))` did not finish in \(Int(timeout)) s"
        }
        let text = String(decoding: output.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        if process.terminationStatus == 0 {
            NSLog("Cua Spaces: %@", text)
            return nil
        }
        return text.isEmpty ? "`cua \(args.joined(separator: " "))` exited with \(process.terminationStatus)" : text
    }

    /// While the returned task runs: every `interval`, `isUp` says whether
    /// this app's daemon answers (another build's does not count); when it
    /// does not, `start` runs again (replacing another build's), backing off
    /// from 2 s to a minute between tries. `report` hears why it could not
    /// (after three tries in a row) and `nil` once it answers again;
    /// `restarted` runs after each start that succeeded (the app connects
    /// again; `start` replaces `cua daemon start` in tests).
    ///
    /// Between probes it watches the daemon's process (`daemonPid`, by
    /// default the pid in `daemon.json`, when it is this app's): its exit
    /// wakes the loop at once (a kqueue exit event, no polling), so a killed
    /// daemon is started again in about a second, not at the next probe.
    ///
    /// `accepted` is the daemon this connection uses when it is another
    /// app's (kept because it is not older than this app's cua): a start
    /// that leaves exactly that daemon running does not connect again, so a
    /// daemon that stopped answering is a failure that backs off, never a
    /// reconnect every interval.
    public func supervise(interval: Duration = .seconds(10),
                          isUp: @escaping @Sendable () async -> Bool,
                          report: @escaping @MainActor @Sendable (String?) -> Void,
                          restarted: (@MainActor @Sendable () async -> Void)? = nil,
                          start: (@Sendable () -> String?)? = nil,
                          daemonPid: (@Sendable () -> Int32?)? = nil,
                          accepted: (@Sendable () async -> UInt32?)? = nil) -> Task<Void, Never> {
        let wake = SupervisorWake()
        let watcher = ProcessExitWatcher { wake.fire() }
        let pidNow: @Sendable () -> Int32? = daemonPid ?? { [self] in
            Self.daemonPids(cuaHome: self.cuaHome).first { self.isOwn(pid: UInt32($0)) }
        }
        return Task.detached(priority: .utility) { [self] in
            defer { watcher.stop() }
            var failures = 0
            var reported = false
            while !Task.isCancelled {
                // A daemon that already exited fires at once.
                if failures == 0, let pid = pidNow() { watcher.watch(pid) }
                await wake.wait(for: failures == 0 ? interval : Self.backoff(failures))
                if Task.isCancelled { return }
                if await isUp() {
                    if reported { await report(nil) }
                    failures = 0
                    reported = false
                    continue
                }
                var error = start.map { $0() } ?? self.start()
                if error == nil, let accepted, let running = Self.daemonPids(cuaHome: self.cuaHome).last,
                   let used = await accepted(), UInt32(running) == used, !self.isOwn(pid: used) {
                    error = "the running cua daemon (pid \(running)) is another app's, which this app uses, and it does not answer"
                }
                if error == nil {
                    // Running now (started again, or it ran and only this
                    // app's connection to it stopped answering): connect
                    // again, which supervises the new connection.
                    if let restarted {
                        await restarted()
                        if Task.isCancelled { return }
                    }
                    if await isUp() {
                        if reported { await report(nil) }
                        failures = 0
                        reported = false
                        continue
                    }
                }
                failures += 1
                NSLog("Cua Spaces: the cua daemon is not answering (try %d): %@", failures, error ?? "no answer")
                if failures >= 3 && !reported {
                    reported = true
                    await report("The Cua daemon stopped and could not be started again: \(error ?? "it does not answer"). "
                                 + "The Keyvault, agents and Cua Volume need it; Cua Spaces keeps trying.")
                }
            }
        }
    }

    /// The wait before try `n` (1, 2, ...): 2 s doubling, at most a minute.
    static func backoff(_ n: Int) -> Duration {
        .seconds(min(60, 2 << min(max(n - 1, 0), 5)))
    }
}

/// Wakes the supervisor's wait early (the daemon exited).
final class SupervisorWake: @unchecked Sendable {
    private let lock = NSLock()
    private var waiting: CheckedContinuation<Void, Never>?
    private var token = 0
    private var fired = false

    /// Wakes the current wait, or the next one if none is running.
    func fire() {
        let c: CheckedContinuation<Void, Never>? = lock.withLock {
            guard let w = waiting else { fired = true; return nil }
            waiting = nil
            return w
        }
        c?.resume()
    }

    /// Returns after `d`, or as soon as `fire` is called.
    func wait(for d: Duration) async {
        let mine: Int = lock.withLock { token += 1; return token }
        await withCheckedContinuation { (c: CheckedContinuation<Void, Never>) in
            let now: Bool = lock.withLock {
                if fired { fired = false; return true }
                waiting = c
                return false
            }
            if now { c.resume(); return }
            Task { [weak self] in
                try? await Task.sleep(for: d)
                guard let self else { return }
                let w: CheckedContinuation<Void, Never>? = self.lock.withLock {
                    guard self.token == mine, let w = self.waiting else { return nil }
                    self.waiting = nil
                    return w
                }
                w?.resume()
            }
        }
    }
}

/// Calls `onExit` when the process it watches exits: a kqueue exit event
/// (`DispatchSource` process source), which needs no parent relationship.
final class ProcessExitWatcher: @unchecked Sendable {
    private let lock = NSLock()
    private var source: DispatchSourceProcess?
    private var pid: Int32?
    private let onExit: @Sendable () -> Void

    init(onExit: @escaping @Sendable () -> Void) { self.onExit = onExit }

    /// Watches `pid` (once; another pid replaces it). A pid already gone
    /// calls `onExit` at once.
    func watch(_ pid: Int32) {
        lock.withLock {
            guard self.pid != pid else { return }
            source?.cancel()
            self.pid = pid
            let s = DispatchSource.makeProcessSource(identifier: pid, eventMask: .exit,
                                                     queue: .global(qos: .utility))
            let onExit = self.onExit
            s.setEventHandler { [weak self, weak s] in
                s?.cancel()
                self?.lock.withLock { if self?.pid == pid { self?.pid = nil; self?.source = nil } }
                onExit()
            }
            source = s
            s.resume()
        }
        // Exited before the source was armed: kqueue would never report it.
        if kill(pid, 0) != 0, errno == ESRCH {
            let gone: Bool = lock.withLock {
                guard self.pid == pid else { return false }
                source?.cancel()
                source = nil
                self.pid = nil
                return true
            }
            if gone { onExit() }
        }
    }

    func stop() {
        lock.withLock {
            source?.cancel()
            source = nil
            pid = nil
        }
    }
}
