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
        /// The pids the supervisor said it watches, in order.
        var watched: [Int32] = []
        /// Whether the supervisor watches the daemon running now.
        var watchingIt: Bool { lock.withLock { process.map { watched.last == $0.processIdentifier } ?? false } }

        func spawn() {
            let p = Process()
            p.executableURL = URL(fileURLWithPath: "/bin/sleep")
            p.arguments = ["60"]
            try? p.run()
            lock.withLock { process = p }
        }
        var pid: Int32? { lock.withLock { process.flatMap { $0.isRunning ? $0.processIdentifier : nil } } }
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
            daemonPid: { daemon.pid },
            watching: { pid in daemon.lock.withLock { daemon.watched.append(pid) } })
        defer { task.cancel() }
        // Killed only once the supervisor watches it (else only the hourly
        // probe could see the kill).
        #expect(await eventually(60) { daemon.watchingIt })
        #expect(daemon.lock.withLock { daemon.starts } == 0)
        // Waiting is not polling.
        #expect(daemon.lock.withLock { daemon.probes } == 0)

        // Started again long before the next probe (an hour): its exit woke
        // the supervisor. (No tighter bound: a loaded run may be slow.)
        daemon.kill()
        #expect(await eventually(60) { daemon.lock.withLock { daemon.starts } == 1 })
        #expect(await eventually(60) { daemon.lock.withLock { daemon.restarted } == 1 })

        // The new daemon is watched too (its own pid, after its confirming
        // probe).
        #expect(await eventually(60) { daemon.watchingIt })
        daemon.kill()
        #expect(await eventually(60) { daemon.lock.withLock { daemon.starts } == 2 })
    }

    /// A daemon started again that dies before the supervisor reads its
    /// pid leaves nothing to watch: it is probed (and started) again at
    /// once, not after the next probe an hour later.
    @Test func aDaemonThatDiesBeforeItIsWatchedIsStartedAgainAtOnce() async throws {
        let supervisor = try #require(DaemonSupervisor(bundledCua: "/bin/sh"))
        let daemon = Daemon()
        daemon.spawn()
        defer { daemon.stop() }
        final class Once: @unchecked Sendable { var done = false }
        let once = Once()
        let task = supervisor.supervise(
            interval: .seconds(3600),
            isUp: {
                let up = daemon.pid != nil
                // The first restart answers its confirming probe, then
                // dies before the supervisor reads its pid.
                let die = daemon.lock.withLock { () -> Bool in
                    guard up, daemon.starts == 1, !once.done else { return false }
                    once.done = true
                    return true
                }
                if die {
                    let p = daemon.lock.withLock { daemon.process }
                    p?.terminate()
                    p?.waitUntilExit()
                }
                return up
            },
            report: { _ in },
            start: {
                daemon.lock.withLock { daemon.starts += 1 }
                daemon.spawn()
                return nil
            },
            daemonPid: { daemon.pid },
            watching: { pid in daemon.lock.withLock { daemon.watched.append(pid) } })
        defer { task.cancel() }
        #expect(await eventually(60) { daemon.watchingIt })
        daemon.kill()
        #expect(await eventually(60) { daemon.lock.withLock { daemon.starts } == 2 })
        #expect(await eventually(60) { daemon.watchingIt })
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
