// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Darwin
import Foundation

/// The Cua Driver page's methods (`driver.*`, `apps/cua-spaces-web/src/
/// bridge/ops/driver.ts`): this Mac's embedded driver ([`EmbeddedDriver`]),
/// read over its socket, with Cua Spaces' own permission check as the truth
/// for Accessibility and Screen Recording (the driver uses the same grants).
@MainActor
final class DriverPageState {
    static let shared = DriverPageState()
    /// The last live test: `DriverLiveTest`.
    var lastTest: [String: Any]?
    /// When the app last restarted a driver whose permission answer was
    /// stale (a running process keeps the Screen Recording answer it had at
    /// launch), so a stuck driver is restarted at most once a minute.
    var lastRestart: Date?
    /// When this app first looked: a driver that isn't up yet is "starting"
    /// for a little while after launch, not "stopped".
    let since = Date()
}

/// The coding agents Set up connects.
private let driverAgents = ["claude-code", "codex"]

extension WebUIBridge {
    func routeDriver(_ method: String, _ args: [String: Any]) async throws -> Any? {
        guard method.hasPrefix("driver.") else { return nil }
        if let machine = args["machine"] as? String, !machine.isEmpty, machine != thisDriverMachineId {
            throw Failure.unsupported("Open Cua Spaces on that computer to see its Cua Driver")
        }
        switch method {
        case "driver.permissions":
            return await appPermissions()
        case "driver.status":
            guard let driver = EmbeddedDriver.current() else { return NSNull() }
            return await driverStatus(driver)
        case "driver.list":
            guard let driver = EmbeddedDriver.current() else { return [Any]() }
            let s = await driverStatus(driver)
            return [["machineId": s["machineId"] ?? "", "name": s["machineName"] ?? "", "os": "macos", "thisMachine": true,
                     "state": s["state"] ?? "stopped", "version": orNull(s["version"]), "update": NSNull()]]
        case "driver.config":
            return try await driverCall(try needDriver(), "get_config")
        case "driver.setConfig":
            let driver = try needDriver()
            guard var changes = args["changes"] as? [String: Any] else { throw Failure.badArgs("changes: {key: value}") }
            // The one key that isn't set_config's.
            if let show = changes.removeValue(forKey: "agent_cursor.enabled") as? Bool {
                _ = try await driverCall(driver, "set_agent_cursor_enabled", ["enabled": show])
            }
            if !changes.isEmpty { _ = try await driverCall(driver, "set_config", changes) }
            return try await driverCall(driver, "get_config")
        case "driver.test":
            return await driverLiveTest(try needDriver())
        case "driver.pause":
            let driver = try needDriver()
            guard let paused = args["paused"] as? Bool else { throw Failure.badArgs("paused: boolean") }
            try setDriverPaused(driver, paused)
            // Switched on: a driver that isn't running starts now, whatever
            // this Mac shares.
            if !paused { await startDriverIfNeeded(driver) }
            return NSNull()
        case "driver.setup":
            let driver = try needDriver()
            // A driver that isn't running starts first, so the page answers
            // even when connecting the agents fails below.
            await startDriverIfNeeded(driver)
            try await setUpDriverAgents(driver, agents: driverAgents)
            // A driver whose permission answer is stale starts again.
            await restartDriverIfStale(driver, force: true)
            _ = await driverLiveTest(driver)
            return await driverStatus(driver)
        case "driver.setApp":
            let driver = try needDriver()
            let app = try string(args, "app")
            guard driverAgents.contains(app) else { throw Failure.badArgs("\(app) brings its own Cua Driver") }
            guard args["on"] as? Bool == true else {
                throw Failure.unsupported("Remove Cua Driver from \(app)'s own settings")
            }
            try await setUpDriverAgents(driver, agents: [app])
            return NSNull()
        default:
            throw Failure.unimplemented(method)
        }
    }

    private var thisDriverMachineId: String { model.host.machineId ?? "this-mac" }

    private func needDriver() throws -> EmbeddedDriver {
        guard let driver = EmbeddedDriver.current() else { throw Failure.unsupported("This build of Cua Spaces carries no Cua Driver") }
        return driver
    }

    private func driverCall(_ driver: EmbeddedDriver, _ tool: String, _ args: [String: Any] = [:]) async throws -> Any {
        let sendable = SendableArgs(args)
        do {
            return try await Task.detached { try driver.call(tool, sendable.value) }.value
        } catch let error as EmbeddedDriver.CallError {
            throw Failure(code: "failed", message: error.message)
        }
    }

    /// `AppPermissions`: Cua Spaces' own Screen Recording and Accessibility,
    /// the grants Cua Driver uses (it runs as Cua Spaces' child, so macOS
    /// never lists it on its own). Answered on any Mac, with or without a
    /// driver, for the first run's permissions step, the Driver page and
    /// This Mac. Once both are on, a running driver that still says no (it
    /// keeps the Screen Recording answer it had at launch) starts again,
    /// once.
    func appPermissions() async -> [String: Any] {
        let states = await PermissionGuide.readStates()
        let granted = PermissionGuide.granted(in: states)
        if granted.contains(.screenRecording), granted.contains(.accessibility), let driver = EmbeddedDriver.current() {
            await restartDriverIfStale(driver, force: false)
        }
        return ["appName": PermissionGuide.appName(), "permissions": PermissionGuide.rows(states: states)]
    }

    /// `DriverStatus`.
    func driverStatus(_ driver: EmbeddedDriver) async -> [String: Any] {
        let paused = FileManager.default.fileExists(atPath: driver.pausedMarker)
        let meta = await Task.detached { driver.metadata() }.value
        let states = await PermissionGuide.readStates()
        let granted = PermissionGuide.granted(in: states)
        let screen = granted.contains(.screenRecording), accessibility = granted.contains(.accessibility)
        var lastError: String?
        let state: String
        if paused {
            state = "paused"
        } else if meta == nil {
            let why = await Task.detached { Self.notRunningReason(driver) }.value
            if why == nil, Date().timeIntervalSince(DriverPageState.shared.since) < 30 {
                state = "starting"
            } else {
                state = "failed"
                lastError = why ?? "Cua Driver isn't answering. Repair starts it again."
            }
        } else if !screen || !accessibility {
            state = "needs-permission"
        } else {
            state = "ready"
            await restartDriverIfStale(driver, force: false)
        }
        var status: [String: Any] = [
            "machineId": thisDriverMachineId,
            "machineName": model.host.computerName() ?? "This Mac",
            "thisMachine": true,
            "state": state,
            "version": orNull(meta?.version),
            "update": NSNull(),
            "lastTest": orNull(DriverPageState.shared.lastTest),
            "lastError": orNull(lastError),
            "permissions": PermissionGuide.rows(states: states),
            "appName": PermissionGuide.appName(),
            "activity": [Any](),
            "canPause": true,
        ]
        status["apps"] = orNull(await driverApps(driver))
        return status
    }

    /// The coding agents on this Mac and whether each runs this driver.
    private func driverApps(_ driver: EmbeddedDriver) async -> [[String: Any]]? {
        guard let setup = model.agentSetup else { return nil }
        var apps: [[String: Any]] = []
        for s in await setup.statuses() where driverAgents.contains(s.id) && s.installed {
            let entry = s.mcpConfig.flatMap { EmbeddedDriver.entry(named: driver.serverName, inConfig: $0) }
            let on = entry.map { driver.isOurs(command: $0.command, args: $0.args) } ?? false
            apps.append(["id": s.id, "name": s.name, "on": on, "own": entry != nil && !on, "bringsOwn": false])
        }
        let t3 = ["/Applications/T3 Code.app", NSHomeDirectory() + "/Applications/T3 Code.app"]
        if t3.contains(where: { FileManager.default.fileExists(atPath: $0) }) {
            apps.append(["id": "t3-code", "name": "T3 Code", "on": false, "own": false, "bringsOwn": true])
        }
        return apps
    }

    /// Both permissions, the screen read, and one agent-pointer move (the
    /// real pointer stays put): `DriverLiveTest`, kept as the last test.
    func driverLiveTest(_ driver: EmbeddedDriver) async -> [String: Any] {
        let started = Date()
        let failure: String? = await Task.detached { () -> String? in
            do {
                let p = try driver.call("check_permissions", ["prompt": false]) as? [String: Any] ?? [:]
                for (key, name) in [("accessibility", "Accessibility"), ("screen_recording", "Screen Recording")] where !Self.granted(p, key) {
                    return "\(name) is off for Cua Spaces"
                }
                _ = try driver.call("get_screen_size")
                _ = try driver.call("move_cursor", ["x": 200, "y": 200, "cursor_id": "cua-spaces-setup-test", "session": "cua-spaces-setup-test"])
                return nil
            } catch let error as EmbeddedDriver.CallError where error.message == "Cua Driver isn't running" {
                return Self.notRunningReason(driver).map { "Cua Driver isn't running. \($0)" } ?? error.message
            } catch {
                return "\(error)"
            }
        }.value
        let test: [String: Any] = [
            "passed": failure == nil,
            "atMs": (started.timeIntervalSince1970 * 1000).rounded(),
            "failure": orNull(failure),
            "tookMs": (Date().timeIntervalSince(started) * 1000).rounded(),
        ]
        DriverPageState.shared.lastTest = test
        return test
    }

    /// `check_permissions` reports `key` granted, as a boolean or `{granted}`.
    nonisolated static func granted(_ permissions: [String: Any], _ key: String) -> Bool {
        if let b = permissions[key] as? Bool { return b }
        return (permissions[key] as? [String: Any])?["granted"] as? Bool ?? false
    }

    /// The driver keeps the Screen Recording answer it had when it started:
    /// granted to Cua Spaces since, it still says no until it starts again.
    /// Its login item starts it again within a second when it exits.
    func restartDriverIfStale(_ driver: EmbeddedDriver, force: Bool) async {
        let state = DriverPageState.shared
        if !force, let last = state.lastRestart, Date().timeIntervalSince(last) < 60 { return }
        let stale: Int32? = await Task.detached { () -> Int32? in
            guard let meta = driver.metadata(), meta.pid > 0,
                  let p = try? driver.call("check_permissions", ["prompt": false]) as? [String: Any] else { return nil }
            return Self.granted(p, "accessibility") && Self.granted(p, "screen_recording") ? nil : meta.pid
        }.value
        guard let pid = stale else { return }
        let granted = await PermissionGuide.readGranted()
        guard granted.contains(.screenRecording), granted.contains(.accessibility) else { return }
        state.lastRestart = Date()
        kill(pid, SIGTERM)
        // Wait for its login item to bring it back (1 s backoff, then startup).
        for _ in 0..<20 {
            try? await Task.sleep(for: .milliseconds(500))
            if let meta = await Task.detached(operation: { driver.metadata(timeout: 1) }).value, meta.pid != pid { return }
        }
    }

    /// Why the driver isn't answering, in plain words, or nil when its
    /// login item is (or may still be) starting it: macOS holds the login
    /// item back, the login item isn't running, or the driver's last start
    /// failed (`DriverSupervisorReport`). Blocks briefly: call off the main
    /// thread.
    nonisolated static func notRunningReason(_ driver: EmbeddedDriver) -> String? {
        guard DriverService.isBundled() else {
            return "This build of Cua Spaces has no Cua Driver login item; install the latest Cua Spaces."
        }
        let item = DriverService.status()
        if item == .requiresApproval { return DriverService.needsApproval }
        let report = DriverSupervisorReport.read(driver.statusFile)
        if item != .enabled || report.map({ !$0.supervisorAlive }) == true {
            return "Cua Driver's background item isn't running. Repair starts it again."
        }
        guard let report else { return nil }
        if let error = report.lastError, report.state != "ready" {
            return "Cua Driver didn't start: \(error)"
        }
        return nil
    }

    /// Gets a driver that isn't answering running, unless the person paused
    /// agents: registers and starts Cua Driver's login item and asks it for
    /// a driver now, then restarts the login item when that didn't work.
    /// Never depends on whether this Mac is set up, shared or signed in
    /// (CUA-3638). Returns once the driver answers or each step had its
    /// time; why it didn't is what `notRunningReason` says.
    func startDriverIfNeeded(_ driver: EmbeddedDriver) async {
        var asked = false
        while true {
            let paused = FileManager.default.fileExists(atPath: driver.pausedMarker)
            let answering = await Task.detached { driver.metadata(timeout: 1) != nil }.value
            switch EmbeddedDriver.startStep(paused: paused, answering: answering, askedLoginItem: asked) {
            case .none:
                return
            case .startLoginItem:
                asked = true
                let request = driver.startRequest
                try? FileManager.default.createDirectory(atPath: (request as NSString).deletingLastPathComponent, withIntermediateDirectories: true)
                FileManager.default.createFile(atPath: request, contents: Data())
                if let why = await Task.detached(operation: { DriverService.start(restart: false) }).value {
                    NSLog("cua-spaces: %@", why)
                    return
                }
                if await waitForDriver(driver, seconds: 15) { return }
            case .restartLoginItem:
                NSLog("cua-spaces: Cua Driver didn't start; restarting its login item")
                _ = await Task.detached { DriverService.start(restart: true) }.value
                _ = await waitForDriver(driver, seconds: 25)
                return
            }
        }
    }

    /// Whether the driver answers within `seconds`.
    private func waitForDriver(_ driver: EmbeddedDriver, seconds: Int) async -> Bool {
        for _ in 0..<(seconds * 2) {
            if await Task.detached(operation: { driver.metadata(timeout: 1) != nil }).value { return true }
            try? await Task.sleep(for: .milliseconds(500))
        }
        return false
    }

    /// "Let agents use apps on this Mac": off writes the file the driver login item
    /// watches, which stops the driver; on removes it.
    private func setDriverPaused(_ driver: EmbeddedDriver, _ paused: Bool) throws {
        let marker = driver.pausedMarker
        if paused {
            try FileManager.default.createDirectory(atPath: (marker as NSString).deletingLastPathComponent, withIntermediateDirectories: true)
            guard FileManager.default.createFile(atPath: marker, contents: Data("1\n".utf8)) else {
                throw Failure(code: "failed", message: "Couldn't pause agents")
            }
        } else if FileManager.default.fileExists(atPath: marker) {
            try FileManager.default.removeItem(atPath: marker)
        }
    }

    /// Set up and Repair: each agent's MCP entry runs this driver through
    /// its embedded proxy, and the cua-driver skill is installed.
    private func setUpDriverAgents(_: EmbeddedDriver, agents: [String]) async throws {
        guard let setup = model.agentSetup else { throw Failure.unsupported("Agent setup is not available in this build") }
        let installed = await setup.statuses().filter { agents.contains($0.id) && $0.installed }.map(\.id)
        guard !installed.isEmpty else { throw Failure(code: "failed", message: "Install Claude Code or Codex first") }
        let outcomes = try await setup.setUpCuaDriver(agents: installed)
        if let failed = outcomes.first(where: { $0.change == "failed" }) {
            throw Failure(code: "failed", message: failed.detail.isEmpty ? "Couldn't set up \(failed.agents.joined(separator: ", "))" : failed.detail)
        }
    }
}

/// A JSON value, or JSON null.
private func orNull(_ value: Any?) -> Any { value ?? NSNull() }

/// Tool arguments are plain JSON values, safe to hand to a detached task.
private struct SendableArgs: @unchecked Sendable {
    let value: [String: Any]
    init(_ value: [String: Any]) { self.value = value }
}
