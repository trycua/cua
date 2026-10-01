// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
@testable import CuaSpacesMacKit
import Foundation
import Testing

/// Records the host setup request; answers with a configured status.
final class FakeHost: HostRunning, @unchecked Sendable {
    var requests: [(AppHostSetupRequest, String?)] = []
    var fail: String?

    func setupRequest(request: AppHostSetupRequest, accountToken: String?) async throws -> HostStatus {
        requests.append((request, accountToken))
        if let fail { throw CuaError.Runtime(message: fail) }
        return HostStatus(
            configured: true, mode: request.mode, relayUrl: request.relayUrl, directUrl: nil,
            envTokenPath: nil, machineId: "m-1", name: request.name, sharing: true,
            serviceInstalled: true, serviceRunning: true, serviceKind: "launchd", online: true,
            clients: [], allow: request.allow ?? [],
            permissions: [HostPermission(id: "accessibility", title: "Accessibility",
                                         settingsUrl: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
                                         instructions: "Turn on Cua Spacesd.")],
            error: nil)
    }

    func status() async throws -> HostStatus { FixtureHost.unconfigured }
    func stopSharing() async throws -> HostStatus { FixtureHost.unconfigured }
    func startSharing() async throws -> HostStatus { FixtureHost.unconfigured }
    func remove() async throws {}
    func configure(change: HostSettingsChange) async throws -> HostStatus { FixtureHost.unconfigured }
}

/// A temp directory with a fake bundled `cua` next to a fake app executable.
/// Nothing outside it is read or written.
struct TempInstall {
    let root: URL
    let home: URL
    let cua: URL

    init() throws {
        root = FileManager.default.temporaryDirectory.appendingPathComponent("cua-mac-cli-\(UUID().uuidString)")
        home = root.appendingPathComponent("home")
        let macos = root.appendingPathComponent("Cua Spaces.app/Contents/MacOS")
        try FileManager.default.createDirectory(at: macos, withIntermediateDirectories: true)
        cua = macos.appendingPathComponent("cua")
        try Data("#!/bin/sh\necho \"cua 1.2.3\"\n".utf8).write(to: cua)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: cua.path)
    }

    /// The SDK's installer (the app core's) over these paths only.
    func installer(bundled: Bool = true) -> AppCliInstaller {
        AppCliInstaller.withPaths(bundled: bundled ? cua.path : nil,
                                  binDir: home.appendingPathComponent(".local/bin").path,
                                  pathEnv: "/usr/bin:/bin",
                                  profile: home.appendingPathComponent(".zshrc").path)
    }

    func remove() { try? FileManager.default.removeItem(at: root) }
}

@MainActor
@Suite("Onboarding writes")
struct OnboardingTests {
    func model(cli: CliInstallRunning? = nil, host: HostRunning? = nil,
               agents: AgentSetupRunning? = nil) -> OnboardingModel {
        let o = OnboardingModel(statePath: nil, cli: cli, host: host, agentSetup: agents)
        o.send(.start)
        return o
    }

    /// A fresh first run in a temp HOME: the bundled cua is installed with
    /// no page, the shell profile puts it on PATH (a new shell resolves it),
    /// and the pages never include a command-line one.
    @Test func firstLaunchInstallsTheCliSilentlyOntoPath() async throws {
        let tmp = try TempInstall()
        defer { tmp.remove() }
        let o = OnboardingModel(statePath: nil, cli: tmp.installer())
        #expect(!o.view.dots.contains { $0.label == "Command line" })
        await o.installCliSilently()
        let target = tmp.home.appendingPathComponent(".local/bin/cua").path
        #expect(FileManager.default.isExecutableFile(atPath: target))
        #expect(o.state.cliTarget == target)
        #expect(o.view.step == .welcome, "no page was shown for it")
        // A new login shell with this HOME and the profile finds `cua`.
        let profile = tmp.home.appendingPathComponent(".zshrc").path
        let sh = Process()
        sh.executableURL = URL(fileURLWithPath: "/bin/sh")
        sh.arguments = ["-c", ". \"$1\" && command -v cua", "sh", profile]
        sh.environment = ["HOME": tmp.home.path, "PATH": "/usr/bin:/bin"]
        let out = Pipe()
        sh.standardOutput = out
        try sh.run()
        sh.waitUntilExit()
        let found = String(decoding: out.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        #expect(sh.terminationStatus == 0)
        #expect(found == target)
        // Walk every page: none is about the command line; Done shows where.
        o.send(.start)
        o.send(.signinDone)
        o.send(.agentsDone(configured: []))
        #expect(o.view.step == .presentation)
        o.send(.presentationDone)
        o.send(.driveContinue)
        o.send(.modeChosen(mode: .client))
        #expect(o.view.summary.first { $0.label == "cua command" }?.value == target)
        // A second launch finds it current and writes nothing new.
        let again = OnboardingModel(statePath: nil, cli: tmp.installer())
        await again.installCliSilently()
        #expect(again.cliPlan?.upToDate == true && again.state.cliTarget == target)
    }

    @Test func withoutABundledCliNothingIsInstalled() async throws {
        let tmp = try TempInstall()
        defer { tmp.remove() }
        let o = OnboardingModel(statePath: nil, cli: tmp.installer(bundled: false))
        await o.installCliSilently()
        #expect(o.state.cliTarget == nil)
        #expect(!FileManager.default.fileExists(atPath: tmp.home.path))
    }

    @Test func thePresentationCardSetsTheNotchSetting() {
        let o = model()
        var saved: [Bool] = []
        o.onPresentation = { saved.append($0) }
        o.send(.signinDone)
        o.send(.agentsDone(configured: []))
        #expect(o.view.presentations.map(\.title) == ["Notch and menu bar", "Menu bar only"])
        #expect(o.view.presentations.map(\.selected) == [true, false], "the notch by default")
        #expect(o.view.presentations.map(\.id) == ["onboarding-notch", "onboarding-menu-bar"])
        #expect(MenuMiniature.icon != nil, "the menu bar icon ships in the bundle")
        o.pickPresentation(menuBar: true)
        #expect(saved == [true])
        #expect(o.view.presentations.map(\.selected) == [false, true])
        o.send(.presentationDone)
        o.send(.driveContinue)
        #expect(o.view.step == .mode)
    }

    func toMode(_ o: OnboardingModel) {
        o.send(.signinDone)
        o.send(.agentsDone(configured: []))
        o.send(.presentationDone)
        o.send(.driveContinue)
    }

    /// Welcome's usage-data switch: from the machine's setting, locked when
    /// the environment decides, and settled before anything is recorded.
    @Test func welcomesUsageSwitchSettlesBeforeAnythingIsRecorded() {
        let telemetry = FixtureTelemetry()
        let o = OnboardingModel(statePath: nil)
        o.telemetry = telemetry
        var left: [Bool] = []
        o.onWelcomeLeft = { left.append($0) }
        o.shown()
        #expect(o.view.usage?.on == true)
        #expect(o.view.usage?.label == "Share anonymous usage data")
        #expect(o.view.notice?.contains("Change it here or in Settings, Privacy.") == true)
        o.setShareUsage(false)
        #expect(o.view.usage?.on == false)
        #expect(telemetry.recorded.isEmpty)
        o.send(.start)
        #expect(left == [false])
        o.send(.signinDone)
        o.finish()
        #expect(telemetry.recorded.isEmpty)

        let locked = FixtureTelemetry()
        locked.current = AppTelemetryInput(enabled: false, lockedBy: "env DO_NOT_TRACK")
        let l = OnboardingModel(statePath: nil)
        l.telemetry = locked
        l.shown()
        #expect(l.view.usage?.on == false)
        #expect(l.view.usage?.enabled == false)
        #expect(l.view.usage?.help == "Set by env DO_NOT_TRACK")
        l.setShareUsage(true)
        #expect(l.view.usage?.on == false)
    }

    @Test func hostSetupRunsWithTheAccountTokenThenFinishes() async {
        let host = FakeHost()
        let o = model(host: host)
        o.accountToken = { "tok-1" }
        toMode(o)
        #expect(o.view.step == .mode)
        #expect(o.view.choices.map(\.mode) == [.client, .host])
        // The telemetry notice and its switch are Welcome's.
        #expect(o.view.notice == nil)
        #expect(o.view.usage == nil)
        o.host.openForm()
        o.host.send(.setName(name: "  studio  "))
        o.host.send(.setAllow(allow: "a@x.com, b@y.com"))
        await o.setUpHost()
        #expect(host.requests.count == 1)
        let (request, token) = host.requests[0]
        #expect(request.mode == "relay")
        #expect(request.relayUrl == "https://relay.cua.ai")
        #expect(request.name == "studio")
        #expect(request.allow == ["a@x.com", "b@y.com"])
        #expect(token == "tok-1")
        #expect(o.view.step == .done)
        #expect(o.state.mode == .host)
        #expect(o.hostPermissions.map(\.id) == ["accessibility"])
        #expect(!o.showingHostForm)
        #expect(o.view.summary.last?.value == "Set up for unattended access")
    }

    @Test func directSetupNeedsAnAddressAndSendsNoToken() async {
        let host = FakeHost()
        let o = model(host: host)
        o.accountToken = { "tok-1" }
        toMode(o)
        o.host.openForm()
        o.host.send(.toggleAdvanced)
        o.host.send(.setDirect(on: true))
        o.host.send(.setListen(listen: "somewhere"))
        #expect(o.host.formView?.canSubmit == false)
        await o.setUpHost()
        #expect(host.requests.isEmpty)
        o.host.send(.setListen(listen: "10.0.0.5:3211"))
        await o.setUpHost()
        #expect(host.requests.first?.0.direct == "10.0.0.5:3211")
        #expect(host.requests.first?.1 == nil)
    }

    @Test func aFailedSetupStaysOnThePageWithTheReason() async {
        let host = FakeHost()
        host.fail = "sign in to Cua first"
        let o = model(host: host)
        toMode(o)
        o.host.openForm()
        await o.setUpHost()
        #expect(o.view.step == .mode)
        #expect(o.host.formView?.error?.contains("sign in to Cua first") == true)
        #expect(o.showingHostForm)
    }

    @Test func theAgentsStepSetsUpTheTickedAgentsOnly() async {
        let agents = FixtureAgentSetup()
        let o = model(agents: agents)
        o.send(.signinDone)
        #expect(o.view.step == .agents)
        await o.loadAgents()
        #expect(o.installedAgents.map(\.id) == ["claude-code", "codex"])
        o.agentSelection.remove("codex")
        await o.setUpAgents()
        #expect(agents.calls == ["setup:claude-code", "driver:claude-code"], "the driver card is on by default")
        #expect(o.agentSummaries.map(\.line) == ["Claude Code: done"])
        o.finishAgents()
        #expect(o.view.step == .presentation)
        #expect(o.state.agents == ["Claude Code"])
    }

    /// The background computer-use card: on by default, like the skills
    /// and MCP switches; ticked, "Set up" also runs the cua-driver step for
    /// the same agents (alone when the skills and MCP switches are off), and
    /// the summary names it.
    @Test func theDriverCardAddsTheCuaDriverStepForTheTickedAgents() async {
        let agents = FixtureAgentSetup()
        let o = model(agents: agents)
        o.send(.signinDone)
        await o.loadAgents()
        #expect(o.agentSkills && o.agentMcp && o.agentDriver, "all three on by default")
        #expect(o.copy.agentsDriver == "cua-driver skill for background computer-use")
        o.agentSkills = false
        o.agentMcp = false
        o.agentDriver = false
        #expect(!o.canSetUpAgents, "nothing chosen")
        o.agentDriver = true
        #expect(o.canSetUpAgents)
        o.agentSelection.remove("codex")
        await o.setUpAgents()
        #expect(agents.calls == ["driver:claude-code"])
        let s = appAgentSetupSummary(outcomes: o.agentOutcomes ?? [], agent: "claude-code", name: "Claude Code")
        #expect(s.text == "cua-driver configured, 1 skill installed")
        o.agentSkills = true
        o.agentMcp = true
        await o.setUpAgents()
        #expect(agents.calls == ["driver:claude-code", "setup:claude-code", "driver:claude-code"])
    }
}
