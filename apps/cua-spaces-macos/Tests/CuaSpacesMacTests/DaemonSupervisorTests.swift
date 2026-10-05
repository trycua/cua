// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesMacKit
import CuaSpacesFFI
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

    /// An in-memory daemon agent.
    final class FakeAgent: DaemonAgentControlling, @unchecked Sendable {
        let label = "com.trycua.spaces.daemon"
        var current: AppLoginItemStatus
        var failure: String?
        var registers = 0
        init(_ status: AppLoginItemStatus) { current = status }
        func status() -> AppLoginItemStatus { current }
        func register() throws {
            registers += 1
            if let failure { throw AgentsToolError(message: failure) }
            current = .enabled
        }
    }

    /// The app's own environment: launchd's `HOME`, no `CUA_*` settings.
    var appEnvironment: [String: String] {
        ["HOME": DaemonSupervisor.accountHome ?? "/", "PATH": "/usr/bin:/bin", "CUA_DAEMON_STARTED_BY": "app"]
    }

    @Test func theDaemonStartsThroughTheAppsAgentOnceRegistered() throws {
        let agent = FakeAgent(.notRegistered)
        let supervisor = try #require(DaemonSupervisor(bundledCua: try fakeCua("exit 0"), agent: agent))
        #expect(supervisor.agentLabel(environment: appEnvironment) == "com.trycua.spaces.daemon")
        #expect(agent.registers == 1)
        // Registered: used as is.
        #expect(supervisor.agentLabel(environment: appEnvironment) == "com.trycua.spaces.daemon")
        #expect(agent.registers == 1)
    }

    @Test func withoutAWorkingAgentTheDaemonIsSpawned() throws {
        let cua = try fakeCua("exit 0")
        #expect(try #require(DaemonSupervisor(bundledCua: cua)).agentLabel(environment: appEnvironment) == nil)
        // Turned off in System Settings, Login Items: never registered again.
        let off = FakeAgent(.requiresApproval)
        #expect(try #require(DaemonSupervisor(bundledCua: cua, agent: off)).agentLabel(environment: appEnvironment) == nil)
        #expect(off.registers == 0)
        let missing = FakeAgent(.notFound)
        #expect(try #require(DaemonSupervisor(bundledCua: cua, agent: missing)).agentLabel(environment: appEnvironment) == nil)
        let failing = FakeAgent(.notRegistered)
        failing.failure = "Operation not permitted"
        #expect(try #require(DaemonSupervisor(bundledCua: cua, agent: failing)).agentLabel(environment: appEnvironment) == nil)
    }

    @Test func aRunWithItsOwnHomeOrSettingsSpawnsTheDaemon() throws {
        let agent = FakeAgent(.enabled)
        let supervisor = try #require(DaemonSupervisor(bundledCua: try fakeCua("exit 0"), agent: agent))
        var env = appEnvironment
        env["CUA_HOME"] = "/tmp/throwaway/.cua"
        #expect(supervisor.agentLabel(environment: env) == nil)
        env = appEnvironment
        env["HOME"] = "/tmp/throwaway"
        #expect(supervisor.agentLabel(environment: env) == nil)
        env = appEnvironment
        env["CUA_DAEMON_LAUNCHD_LABEL"] = "stale"
        #expect(supervisor.agentLabel(environment: env) == "com.trycua.spaces.daemon")
    }

    @Test func startPassesTheAgentsLabelToDaemonStart() throws {
        let cua = try fakeCua(#"[ "$CUA_DAEMON_LAUNCHD_LABEL" = "$EXPECT_LABEL" ] || { echo "label '$CUA_DAEMON_LAUNCHD_LABEL'"; exit 1; }"#)
        // This test process has its own environment; the label depends on
        // it (a `CUA_*` setting or another HOME spawns), so expect what
        // agentLabel says for it.
        let supervisor = try #require(DaemonSupervisor(bundledCua: cua, agent: FakeAgent(.enabled)))
        let expected = supervisor.agentLabel(environment: ProcessInfo.processInfo.environment) ?? ""
        setenv("EXPECT_LABEL", expected, 1)
        defer { unsetenv("EXPECT_LABEL") }
        #expect(supervisor.start() == nil)
        setenv("EXPECT_LABEL", "", 1)
        #expect(try #require(DaemonSupervisor(bundledCua: cua)).start() == nil)
    }

    /// The agent the app registers runs what `cua daemon start` would spawn
    /// (`cua daemon start --foreground` from this bundle, default
    /// loopback), which is when the CLI hands the start to launchd.
    @Test func theBundledAgentRunsTheBundlesDaemon() throws {
        let url = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().appendingPathComponent("Support/\(LiveDaemonAgent.plistName)")
        let plist = try #require(try PropertyListSerialization.propertyList(
            from: Data(contentsOf: url), format: nil) as? [String: Any])
        #expect(plist["Label"] as? String == LiveDaemonAgent().label)
        #expect(plist["BundleProgram"] as? String == "Contents/MacOS/cua")
        #expect(plist["ProgramArguments"] as? [String]
                == ["cua", "daemon", "start", "--foreground", "--loopback", "127.0.0.1:0"])
        #expect((plist["EnvironmentVariables"] as? [String: String])?["CUA_DAEMON_STARTED_BY"] == "app")
        // Started on demand only: the app's login item brings it back, and
        // `cua daemon stop` must stay stopped.
        #expect(plist["RunAtLoad"] as? Bool == false)
        #expect(plist["KeepAlive"] as? Bool == false)
    }

    @Test func restartsBackOffToAMinute() {
        #expect(DaemonSupervisor.backoff(1) == .seconds(2))
        #expect(DaemonSupervisor.backoff(2) == .seconds(4))
        #expect(DaemonSupervisor.backoff(5) == .seconds(32))
        #expect(DaemonSupervisor.backoff(6) == .seconds(60))
        #expect(DaemonSupervisor.backoff(40) == .seconds(60))
    }
}
