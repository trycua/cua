// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpacesFFI
import Foundation

/// This app's own `cua daemon`: the bundled `cua`'s build. The Keyvault, the
/// persistent-agent supervisor, Cua Volume and host Spaces live only in the
/// daemon, so the app starts it at launch, has it replace a daemon of
/// another build (another app's, or its own from before an update), and
/// starts it again when it dies. `cua daemon start` does the work: it
/// starts a daemon that survives the app (as the Tauri app's does), replaces
/// a stranger, and does nothing when this build's already runs.
public final class DaemonSupervisor: @unchecked Sendable {
    /// The bundled `cua`.
    public let cua: String
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
    /// daemon runs afterwards, else why not.
    public func start(timeout: TimeInterval = 30) -> String? {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: cua)
        process.arguments = ["daemon", "start"]
        var env = ProcessInfo.processInfo.environment
        env["CUA_DAEMON_STARTED_BY"] = "app"
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

    /// The wait before try `n` (1, 2, ...): 2 s doubling, at most a minute.
    static func backoff(_ n: Int) -> Duration {
        .seconds(min(60, 2 << min(max(n - 1, 0), 5)))
    }
}
