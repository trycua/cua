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
        var startedAt: [Date] = []

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
        // A probe every minute: only the exit can wake it in time.
        let task = supervisor.supervise(
            interval: .seconds(60),
            isUp: {
                daemon.lock.withLock { daemon.probes += 1 }
                return daemon.pid != nil
            },
            report: { _ in },
            restarted: { daemon.lock.withLock { daemon.restarted += 1 } },
            start: {
                daemon.lock.withLock { daemon.starts += 1; daemon.startedAt.append(Date()) }
                daemon.spawn()
                return nil
            },
            daemonPid: { daemon.pid })
        defer { task.cancel() }
        try await Task.sleep(for: .milliseconds(300))
        #expect(daemon.lock.withLock { daemon.starts } == 0)
        // Waiting is not polling.
        #expect(daemon.lock.withLock { daemon.probes } == 0)

        let killed = Date()
        daemon.kill()
        #expect(await eventually(20) { daemon.lock.withLock { daemon.starts } == 1 })
        let took = try #require(daemon.lock.withLock { daemon.startedAt.first }).timeIntervalSince(killed)
        #expect(took < 2, "started again after \(took) s")
        #expect(await eventually(20) { daemon.lock.withLock { daemon.restarted } == 1 })

        // The new daemon is watched too.
        try await Task.sleep(for: .milliseconds(200))
        let again = Date()
        daemon.kill()
        #expect(await eventually(20) { daemon.lock.withLock { daemon.starts } == 2 })
        let second = try #require(daemon.lock.withLock { daemon.startedAt.last }).timeIntervalSince(again)
        #expect(second < 2, "started again after \(second) s")
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

    @Test func aWakeBeforeTheWaitEndsItAtOnce() async {
        let wake = SupervisorWake()
        wake.fire()
        let start = Date()
        await wake.wait(for: .seconds(30))
        #expect(Date().timeIntervalSince(start) < 2)
        // Without a wake, it waits its time.
        let t = Date()
        await wake.wait(for: .milliseconds(150))
        #expect(Date().timeIntervalSince(t) >= 0.1)
    }
}
