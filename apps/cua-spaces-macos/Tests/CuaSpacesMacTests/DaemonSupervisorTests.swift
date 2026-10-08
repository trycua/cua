// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesMacKit
import Foundation
import Testing

/// This app's daemon lifecycle: `cua daemon start` bounded and its outcome
/// read, and the restart backoff. A stand-in `cua` script; no daemon runs.
@Suite struct DaemonSupervisorTests {
    /// A throwaway executable `cua` running `body`.
    func fakeCua(_ body: String) throws -> String {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("cua-supervisor-\(UUID().uuidString)/Cua Spaces.app/Contents/MacOS")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let path = dir.appendingPathComponent("cua").path
        try "#!/bin/sh\n\(body)\n".write(toFile: path, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: path)
        return path
    }

    @Test func aBundleWithoutCuaHasNoSupervisor() {
        #expect(DaemonSupervisor(bundledCua: "/nonexistent/Cua Spaces.app/Contents/MacOS/cua") == nil)
    }

    @Test func startRunsDaemonStartAndReadsItsOutcome() throws {
        let ok = try #require(DaemonSupervisor(bundledCua: try fakeCua(
            #"[ "$1 $2" = "daemon start" ] && [ "$CUA_DAEMON_STARTED_BY" = app ] && echo "cua daemon started (pid 1)""#)))
        #expect(ok.start() == nil)
        let failing = try #require(DaemonSupervisor(bundledCua: try fakeCua("echo 'the daemon did not start within 10 s' >&2; exit 1")))
        #expect(failing.start() == "the daemon did not start within 10 s")
        let hung = try #require(DaemonSupervisor(bundledCua: try fakeCua("exec sleep 30")))
        #expect(hung.start(timeout: 1) == "`cua daemon start` did not finish in 1 s")
    }

    @Test func onlyAProcessRunningFromThisBundleIsItsOwnDaemon() throws {
        let supervisor = try #require(DaemonSupervisor(bundledCua: try fakeCua("exit 0")))
        #expect(supervisor.bundle.hasSuffix("/Cua Spaces.app"))
        // This test process runs from its own bundle, not the fake app's.
        #expect(!supervisor.isOwn(pid: UInt32(getpid())))
        #expect(!supervisor.isOwn(pid: 0))
    }

    /// Another app's daemon of the same or a newer version, which this
    /// connection uses, stopped answering and `cua daemon start` kept it:
    /// no "started again" reconnect every interval, a failure that backs off.
    @Test func aStartThatKeepsTheOtherAppsDaemonInUseDoesNotReconnect() async throws {
        let supervisor = try #require(DaemonSupervisor(bundledCua: try fakeCua("exit 0")))
        let home = FileManager.default.temporaryDirectory.appendingPathComponent("cua-home-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: home) }
        // This test process: alive, and not the fake app's own daemon.
        try Data(#"{"pid":\#(getpid())}"#.utf8).write(to: home.appendingPathComponent("daemon.json"))
        supervisor.homeOverride = home
        let counts = Counts()
        let task = supervisor.supervise(
            interval: .milliseconds(10),
            isUp: { false },
            report: { _ in },
            restarted: { counts.add(reconnect: true) },
            start: { counts.add(reconnect: false); return nil },
            daemonPid: { nil },
            accepted: { UInt32(getpid()) })
        try await Task.sleep(for: .milliseconds(300))
        task.cancel()
        // One start after 10 ms, the next only after the 2 s backoff.
        #expect(counts.value == (starts: 1, reconnects: 0))

        // A different daemon than the one in use (this app's is gone and the
        // other app's runs): the start keeps it and the app connects to it.
        let again = Counts()
        let task2 = supervisor.supervise(
            interval: .milliseconds(10),
            isUp: { again.value.reconnects > 0 },
            report: { _ in },
            restarted: { again.add(reconnect: true) },
            start: { again.add(reconnect: false); return nil },
            daemonPid: { nil },
            accepted: { 1 })
        // The reconnect runs on the main actor, which other suites share.
        for _ in 0..<500 where again.value.reconnects == 0 { try await Task.sleep(for: .milliseconds(10)) }
        try await Task.sleep(for: .milliseconds(100))
        task2.cancel()
        #expect(again.value == (starts: 1, reconnects: 1))
    }

    final class Counts: @unchecked Sendable {
        private let lock = NSLock()
        private var starts = 0
        private var reconnects = 0
        func add(reconnect: Bool) {
            lock.lock()
            if reconnect { reconnects += 1 } else { starts += 1 }
            lock.unlock()
        }
        var value: (starts: Int, reconnects: Int) {
            lock.lock()
            defer { lock.unlock() }
            return (starts, reconnects)
        }
    }

    @Test func restartsBackOffToAMinute() {
        #expect(DaemonSupervisor.backoff(1) == .seconds(2))
        #expect(DaemonSupervisor.backoff(2) == .seconds(4))
        #expect(DaemonSupervisor.backoff(5) == .seconds(32))
        #expect(DaemonSupervisor.backoff(6) == .seconds(60))
        #expect(DaemonSupervisor.backoff(40) == .seconds(60))
    }
}
