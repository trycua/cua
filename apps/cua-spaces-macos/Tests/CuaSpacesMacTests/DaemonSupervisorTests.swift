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

    /// A stand-in `cua daemon start` with cua-daemon's rule: it replaces
    /// (kills) the running daemon unless `CUA_DAEMON_KEEP` names it.
    func ruleCua(home: URL, log: String) throws -> String {
        try fakeCua(#"""
            echo "keep=$CUA_DAEMON_KEEP" >> '\#(log)'
            f='\#(home.appendingPathComponent("daemon.json").path)'
            pid=$(sed -n 's/.*"pid":\([0-9]*\).*/\1/p' "$f" 2>/dev/null)
            [ -n "$pid" ] || exit 0
            case ":$CUA_DAEMON_KEEP:" in *:/bin/sleep:*) echo "using it" ;; *) kill $pid; rm -f "$f"; echo "replacing" ;; esac
            """#)
    }

    /// Another app's daemon: `/bin/sleep`, named in `home`'s `daemon.json`.
    func otherDaemon(home: URL) throws -> Process {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/bin/sleep")
        p.arguments = ["30"]
        try p.run()
        try Data(#"{"pid":\#(p.processIdentifier)}"#.utf8).write(to: home.appendingPathComponent("daemon.json"))
        return p
    }

    /// An older app restarts its own daemon whenever it is replaced: it is
    /// replaced once, then kept when it comes back (`CUA_DAEMON_KEEP`), and
    /// the app says so; nothing is remembered in the normal cases.
    @Test func anotherAppsDaemonIsReplacedOnceThenKept() throws {
        let home = FileManager.default.temporaryDirectory.appendingPathComponent("cua-home-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: home) }
        let log = home.appendingPathComponent("cua.log").path
        let supervisor = try #require(DaemonSupervisor(bundledCua: try ruleCua(home: home, log: log)))
        supervisor.homeOverride = home
        let published = Published()
        supervisor.publishKeep = { published.set($0) }

        // Nothing runs: nothing remembered.
        #expect(supervisor.start() == nil)
        #expect(published.value == nil)

        // Replaced once.
        let first = try otherDaemon(home: home)
        #expect(supervisor.start() == nil)
        first.waitUntilExit()
        #expect(published.value == "/bin/sleep")
        // Its app starts it again: kept, every time after.
        let back = try otherDaemon(home: home)
        defer { back.terminate() }
        #expect(supervisor.start() == nil)
        #expect(supervisor.start() == nil)
        #expect(back.isRunning)
        let lines = try String(contentsOfFile: log, encoding: .utf8).split(separator: "\n").map(String.init)
        #expect(lines == ["keep=", "keep=", "keep=/bin/sleep", "keep=/bin/sleep"])
        // And the app says so.
        #expect(supervisor.yieldsTo(pid: UInt32(back.processIdentifier)) == "/bin/sleep")
        #expect(supervisor.yieldsTo(pid: UInt32(getpid())) == nil)
        let notice = DaemonSupervisor.yieldNotice(executable: "/Applications/Cua Spaces.app/Contents/MacOS/cua", version: "0.4.0")
        #expect(notice.hasPrefix("Another Cua Spaces app (/Applications/Cua Spaces.app, cua 0.4.0) is running and keeps starting its own daemon"))
        #expect(notice.contains("Quit the other app, then reopen Cua Spaces"))
    }

    final class Published: @unchecked Sendable {
        private let lock = NSLock()
        private var list: String?
        func set(_ v: String) { lock.withLock { list = v } }
        var value: String? { lock.withLock { list } }
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
