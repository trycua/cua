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

    @Test func restartsBackOffToAMinute() {
        #expect(DaemonSupervisor.backoff(1) == .seconds(2))
        #expect(DaemonSupervisor.backoff(2) == .seconds(4))
        #expect(DaemonSupervisor.backoff(5) == .seconds(32))
        #expect(DaemonSupervisor.backoff(6) == .seconds(60))
        #expect(DaemonSupervisor.backoff(40) == .seconds(60))
    }
}
