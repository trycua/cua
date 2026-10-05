// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpacesFFI
import Foundation
import ServiceManagement

/// This app's daemon as its launchd agent
/// (`Contents/Library/LaunchAgents/com.trycua.spaces.daemon.plist`).
/// Everything that registers or reads it goes through this protocol, so
/// tests never touch the Mac's real launchd agents.
public protocol DaemonAgentControlling: AnyObject, Sendable {
    /// The agent's launchd label.
    var label: String { get }
    /// What the system reports now.
    func status() -> AppLoginItemStatus
    /// Registers it (launchd loads it; it runs when started).
    func register() throws
}

/// `SMAppService.agent`: macOS holds this app responsible for the agent, so
/// the daemon it runs answers to the app's Local Network permission even
/// after the app quits.
public final class LiveDaemonAgent: DaemonAgentControlling, @unchecked Sendable {
    public static let plistName = "com.trycua.spaces.daemon.plist"
    public let label = "com.trycua.spaces.daemon"
    private let service = SMAppService.agent(plistName: LiveDaemonAgent.plistName)

    public init() {}

    public func status() -> AppLoginItemStatus { MainAppLoginItem.map(service.status) }
    public func register() throws { try service.register() }
}

/// This app's own `cua daemon`: the bundled `cua`'s build. The Keyvault, the
/// persistent-agent supervisor, Cua Volume and host Spaces live only in the
/// daemon, so the app starts it at launch, has it replace a daemon of
/// another build (another app's, or its own from before an update), and
/// starts it again when it dies. `cua daemon start` does the work: it
/// starts a daemon that survives the app (as the Tauri app's does), replaces
/// a stranger, and does nothing when this build's already runs.
///
/// With this app's launchd agent registered, `cua daemon start` has launchd
/// run the daemon (`CUA_DAEMON_LAUNCHD_LABEL`). Spawned as a child of the
/// app, the daemon answered to the app process for Local Network access and
/// could lose it when the app quit ("No route to host" creating a Space).
/// Without the agent (the user turned it off, a development build) it is
/// spawned as before.
public final class DaemonSupervisor: @unchecked Sendable {
    /// The bundled `cua`.
    public let cua: String
    /// This app's bundle (`<App>.app`, holding `Contents/MacOS/cua`).
    public let bundle: String

    /// This app's daemon agent; `nil` spawns the daemon as a child.
    let agent: DaemonAgentControlling?

    /// `nil` when the bundle has no executable `cua` (a bare build).
    public init?(bundledCua: String, agent: DaemonAgentControlling? = nil) {
        guard FileManager.default.isExecutableFile(atPath: bundledCua) else { return nil }
        cua = bundledCua
        self.agent = agent
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
    public func start(timeout: TimeInterval = 60) -> String? {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: cua)
        process.arguments = ["daemon", "start"]
        var env = ProcessInfo.processInfo.environment
        env["CUA_DAEMON_STARTED_BY"] = "app"
        env["CUA_DAEMON_LAUNCHD_LABEL"] = agentLabel(environment: env)
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
            return "`cua daemon start` did not finish in \(Int(timeout)) s"
        }
        let text = String(decoding: output.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        if process.terminationStatus == 0 {
            NSLog("Cua Spaces: %@", text)
            return nil
        }
        return text.isEmpty ? "`cua daemon start` exited with \(process.terminationStatus)" : text
    }

    /// While the returned task runs: every `interval`, `isUp` says whether
    /// this app's daemon answers (another build's does not count); when it
    /// does not, `start` runs again (replacing another build's), backing off
    /// from 2 s to a minute between tries. `report` hears why it could not
    /// (after three tries in a row) and `nil` once it answers again.
    public func supervise(interval: Duration = .seconds(10),
                          isUp: @escaping @Sendable () async -> Bool,
                          report: @escaping @MainActor @Sendable (String?) -> Void) -> Task<Void, Never> {
        Task.detached(priority: .utility) { [self] in
            var failures = 0
            var reported = false
            while !Task.isCancelled {
                try? await Task.sleep(for: failures == 0 ? interval : Self.backoff(failures))
                if Task.isCancelled { return }
                if await isUp() {
                    if reported { await report(nil) }
                    failures = 0
                    reported = false
                    continue
                }
                let error = self.start()
                if error == nil, await isUp() {
                    if reported { await report(nil) }
                    failures = 0
                    reported = false
                    continue
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

    /// The launchd label `cua daemon start` should start the daemon with:
    /// the agent's, once registered (registering it the first time). `nil`
    /// (spawn it) when there is no agent, it waits for the user's approval
    /// in System Settings, registering fails, or this run points the
    /// daemon at another home or state (`HOME`, `CUA_*` settings: tests,
    /// development runs), which the agent's fixed environment would not carry.
    func agentLabel(environment: [String: String]) -> String? {
        guard let agent else { return nil }
        if environment.keys.contains(where: { $0.hasPrefix("CUA_") && !Self.agentCarries.contains($0) })
            || environment["HOME"].map({ $0 != Self.accountHome }) == true {
            return nil
        }
        if agent.status() == .notRegistered {
            do { try agent.register() } catch {
                NSLog("Cua Spaces: the daemon agent was not registered: %@", error.localizedDescription)
                return nil
            }
        }
        return agent.status() == .enabled ? agent.label : nil
    }

    /// This user's home from the account database (launchd's `HOME`), not
    /// from this process's environment.
    static let accountHome: String? = getpwuid(getuid()).flatMap { $0.pointee.pw_dir.map { String(cString: $0) } }

    /// The `CUA_*` variables a launchd-run daemon still gets right: set by
    /// the agent's property list, or by this supervisor.
    static let agentCarries: Set<String> = ["CUA_DAEMON_STARTED_BY", "CUA_DAEMON_LAUNCHD_LABEL"]

    /// The wait before try `n` (1, 2, ...): 2 s doubling, at most a minute.
    static func backoff(_ n: Int) -> Duration {
        .seconds(min(60, 2 << min(max(n - 1, 0), 5)))
    }
}
