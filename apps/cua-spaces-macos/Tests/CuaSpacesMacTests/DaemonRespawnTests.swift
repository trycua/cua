// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesMacKit
import Foundation
import Testing

/// A killed daemon is seen and started again in about a second (it once took
/// 8–10 s, the probe interval), from its process's exit, not by polling.
/// The "daemon" is a `sleep` this test starts; nothing real is touched.
@Suite struct DaemonRespawnTests {
    final class Daemon: @unchecked Sendable {
        let lock = NSLock()
        var process: Process?
        var starts = 0
        var probes = 0
        var restarted = 0
        /// How often the supervisor read the pid it arms its exit watch on.
        var pidReads = 0

        func spawn() {
            let p = Process()
            p.executableURL = URL(fileURLWithPath: "/bin/sleep")
            p.arguments = ["60"]
            try? p.run()
            lock.withLock { process = p }
        }
        var pid: Int32? { lock.withLock { process.flatMap { $0.isRunning ? $0.processIdentifier : nil } } }
        /// The supervisor's `daemonPid`: it watches the pid this returns. The
        /// pid is captured and the read counted under one lock, so a test
        /// that sees the count cannot kill the daemon before the pid is
        /// captured (which would return nil and arm no watch).
        func watchedPid() -> Int32? {
            lock.withLock {
                pidReads += 1
                return process.flatMap { $0.isRunning ? $0.processIdentifier : nil }
            }
        }
        func kill() { lock.withLock { process }?.terminate() }
        func stop() { lock.withLock { process }?.terminate() }
    }

    func eventually(_ seconds: Double, _ condition: () -> Bool) async -> Bool {
        let deadline = Date().addingTimeInterval(seconds)
        while Date() < deadline {
            if condition() { return true }
            try? await Task.sleep(for: .milliseconds(20))
        }
        return condition()
    }

    @Test func aKilledDaemonIsStartedAgainAtOnceWithoutPolling() async throws {
        let supervisor = try #require(DaemonSupervisor(bundledCua: "/bin/sh"))
        let daemon = Daemon()
        daemon.spawn()
        defer { daemon.stop() }
        // A probe every hour: only the exit can wake it in time.
        let task = supervisor.supervise(
            interval: .seconds(3600),
            isUp: {
                daemon.lock.withLock { daemon.probes += 1 }
                return daemon.pid != nil
            },
            report: { _ in },
            restarted: { daemon.lock.withLock { daemon.restarted += 1 } },
            start: {
                daemon.lock.withLock { daemon.starts += 1 }
                daemon.spawn()
                return nil
            },
            daemonPid: { daemon.watchedPid() })
        defer { task.cancel() }
        // The supervisor has read the pid and armed its watch (a kill before
        // that is not seen until the next probe), then stays quiet.
        #expect(await eventually(60) { daemon.lock.withLock { daemon.pidReads } >= 1 })
        try await Task.sleep(for: .milliseconds(300))
        #expect(daemon.lock.withLock { daemon.starts } == 0)
        // Waiting is not polling.
        #expect(daemon.lock.withLock { daemon.probes } == 0)

        // Started again long before the next probe (an hour): its exit woke
        // the supervisor. (No tighter bound: a loaded run may be slow.)
        daemon.kill()
        #expect(await eventually(60) { daemon.lock.withLock { daemon.starts } == 1 })
        #expect(await eventually(60) { daemon.lock.withLock { daemon.restarted } == 1 })

        // The new daemon is watched too. The supervisor reads the pid to
        // watch only after its confirming probe (the second one: the miss,
        // then this one), at the top of its loop; a kill before that read
        // finds no pid to watch and is never seen until the next hourly
        // probe. So wait for the second read (the first was at the start),
        // not for a fixed time: the watch is armed in the same step.
        #expect(await eventually(60) { daemon.lock.withLock { daemon.pidReads } >= 2 })
        daemon.kill()
        #expect(await eventually(60) { daemon.lock.withLock { daemon.starts } == 2 })
    }

    @Test func anExitWatcherFiresForAProcessThatIsAlreadyGone() async {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/usr/bin/true")
        try? p.run()
        p.waitUntilExit()
        let fired = NSLock()
        nonisolated(unsafe) var count = 0
        let watcher = ProcessExitWatcher { fired.withLock { count += 1 } }
        watcher.watch(p.processIdentifier)
        #expect(await eventually(5) { fired.withLock { count } == 1 })
        watcher.stop()
    }

    /// A wait of an hour that a wake before it ends (the time limit fails
    /// the test otherwise).
    @Test(.timeLimit(.minutes(2))) func aWakeBeforeTheWaitEndsItAtOnce() async {
        let wake = SupervisorWake()
        wake.fire()
        await wake.wait(for: .seconds(3600))
        // Without a wake, it waits its time.
        let t = Date()
        await wake.wait(for: .milliseconds(150))
        #expect(Date().timeIntervalSince(t) >= 0.1)
    }
}
