// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
@testable import CuaSpacesMacKit
import CuaSpacesFFI
import Foundation
import Testing

/// New UI (preview): the bundled web UI's scheme handler, the bridge's
/// value encoding and its envelope.
@Suite("Web UI host")
@MainActor
struct WebUIHostTests {
    @Test func theSchemeHandlerServesOnlyTheWebUIDirectory() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("webui-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: root.appendingPathComponent("assets"), withIntermediateDirectories: true)
        try Data("<html></html>".utf8).write(to: root.appendingPathComponent("index.html"))
        try Data("x".utf8).write(to: root.appendingPathComponent("assets/app.js"))
        defer { try? FileManager.default.removeItem(at: root) }
        let h = WebUISchemeHandler(root: root)
        let r = root.standardizedFileURL
        #expect(h.resolve("/", in: r)?.lastPathComponent == "index.html")
        #expect(h.resolve("/assets/app.js", in: r)?.lastPathComponent == "app.js")
        // A client-side route gets the app; a missing asset does not.
        #expect(h.resolve("/spaces/abc", in: r)?.lastPathComponent == "index.html")
        #expect(h.resolve("/assets/missing.js", in: r) == nil)
        // Nothing outside the directory.
        #expect(h.resolve("/../../etc/passwd", in: r) == nil)
        #expect(h.resolve("/%2e%2e/secret.txt", in: r) == nil)
        #expect(WebUISchemeHandler.mime("js") == "text/javascript")
        #expect(WebUISchemeHandler.mime("wasm") == "application/wasm")
    }

    @Test func coreRecordsEncodeAsPlainValues() {
        let x = AppExperiments(cuaVolume: true, yourCloud: false, sharing: false, webUi: true)
        let v = BridgeValue.encode(x) as? [String: Any]
        #expect(v?["cuaVolume"] as? Bool == true)
        #expect(v?["webUi"] as? Bool == true)
        #expect(BridgeValue.encode(Optional<String>.none) is NSNull)
        #expect(BridgeValue.encode([1, 2]) as? [Int] == [1, 2])
        #expect(BridgeValue.encode(AppSignInPhase.idle) as? String == "idle")
        let waiting = BridgeValue.encode(AppSignInPhase.waiting(userCode: "AB-12")) as? [String: Any]
        #expect(waiting?["type"] as? String == "waiting")
        #expect(waiting?["userCode"] as? String == "AB-12")
    }

    @Test func anUnknownMethodIsUnimplemented() async {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("cua-webui-\(UUID().uuidString)")
        let model = AppModel(backend: FixtureSpacesBackend(), keyvault: KeyvaultModel(client: nil),
                             onboarding: OnboardingModel(statePath: dir.appendingPathComponent("onboarding.json").path),
                             settingsPath: dir.appendingPathComponent("settings.json").path,
                             telemetry: FixtureTelemetry())
        let bridge = WebUIBridge(model: model, allowedOrigins: [])
        do {
            _ = try await bridge.handle("spaces.teleport", [:])
            Issue.record("expected unimplemented")
        } catch let f as WebUIBridge.Failure {
            #expect(f.code == "unimplemented")
        } catch {
            Issue.record("\(error)")
        }
        let info = try? await bridge.handle("app.info", [:]) as? [String: Any]
        #expect(info?["platform"] as? String == "macos")
        #expect((info?["methods"] as? [String])?.contains("keyvault.unlock") == true)
        // The web UI's vault list reads the overview's items.
        let kv = try? await bridge.handle("keyvault.get", [:]) as? [String: Any]
        #expect(kv?["overview"] is [String: Any])
        #expect((kv?["overview"] as? [String: Any])?["items"] is [Any])
        let reply = WebUIBridge.reply(id: "7", error: .badArgs("id: string"))
        #expect(reply["ok"] as? Bool == false)
        #expect((reply["error"] as? [String: String])?["code"] == "bad_args")
    }

    func bridge(kv: KeyvaultClientProtocol? = nil, backend: SpacesBackend = FixtureSpacesBackend(),
                host: HostRunning? = nil, agentSetup: AgentSetupRunning? = nil) -> (WebUIBridge, AppModel) {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("cua-webui-\(UUID().uuidString)")
        let model = AppModel(backend: backend, keyvault: KeyvaultModel(client: kv, clock: { fixtureNow }),
                             onboarding: OnboardingModel(statePath: dir.appendingPathComponent("onboarding.json").path),
                             settingsPath: dir.appendingPathComponent("settings.json").path,
                             host: host, telemetry: FixtureTelemetry(), agentSetup: agentSetup)
        return (WebUIBridge(model: model, allowedOrigins: []), model)
    }

    func failure(_ bridge: WebUIBridge, _ method: String, _ args: [String: Any]) async -> String? {
        do {
            _ = try await bridge.handle(method, args)
            return nil
        } catch let f as WebUIBridge.Failure {
            return f.code
        } catch {
            return "\(error)"
        }
    }

    /// A Space's thumbnail crosses as a JPEG `data:` URL from the same
    /// store the notch and the native cover read; an unknown Space is not
    /// found.
    @Test func aSpacesThumbnailCrossesAsADataURL() async throws {
        let (bridge, model) = bridge()
        await model.refresh()
        let id = try #require(model.spaces.first?.id)
        let image = NSImage(size: NSSize(width: 8, height: 5), flipped: false) { r in
            NSColor.systemTeal.setFill()
            r.fill()
            return true
        }
        // Newer than anything the fixture backend answers, so it stays.
        let at = Date(timeIntervalSince1970: 4_102_444_800)
        model.thumbnails.set(id, image, at: at)
        let out = try await bridge.handle("spaces.thumbnail", ["spaceId": id, "maxAgeMs": 60_000]) as? [String: Any]
        #expect((out?["url"] as? String)?.hasPrefix("data:image/jpeg;base64,") == true)
        #expect(out?["capturedAtMs"] as? Double == at.timeIntervalSince1970 * 1000)
        #expect(await failure(bridge, "spaces.thumbnail", ["spaceId": "local:nope"]) == "not_found")
        #expect(await failure(bridge, "spaces.thumbnail", [:]) == "bad_args")
    }

    /// The drop well: only files picked with "Send file…" or dropped on the
    /// window (read from the drag's own pasteboard, matched by the names
    /// the page saw) are sent.
    @Test func aSpacesDropWellSendsOnlyWhatWasPickedOrDropped() async throws {
        let (bridge, model) = bridge()
        await model.refresh()
        let id = try #require(model.spaces.first?.id)
        bridge.pickFiles = { [URL(fileURLWithPath: "/tmp/notes.txt")] }
        #expect(try await bridge.handle("spaces.chooseFiles", [:]) as? [String] == ["/tmp/notes.txt"])
        let sent = try await bridge.handle("spaces.sendFiles", ["spaceId": id, "paths": ["/tmp/notes.txt"]]) as? [[String: Any]]
        #expect(sent?.first?["name"] as? String == "notes.txt")
        #expect(await failure(bridge, "spaces.sendFiles", ["spaceId": id, "paths": ["/etc/passwd"]]) == "forbidden")
        #expect(await failure(bridge, "spaces.sendFiles", ["spaceId": id, "paths": []]) == "bad_args")

        let board = NSPasteboard(name: NSPasteboard.Name("cua-test-drag-\(UUID().uuidString)"))
        defer { board.releaseGlobally() }
        board.clearContents()
        board.writeObjects([URL(fileURLWithPath: "/tmp/a.txt") as NSURL, URL(fileURLWithPath: "/Applications/Calculator.app") as NSURL])
        bridge.dragPasteboard = { board }
        // Only the names the page saw dropped (an older drag's never match).
        #expect(try await bridge.handle("spaces.droppedFiles", ["names": ["a.txt"]]) as? [String] == ["/tmp/a.txt"])
        #expect(try await bridge.handle("spaces.droppedFiles", ["names": ["other.txt"]]) as? [String] == [])
        #expect(try await bridge.handle("spaces.sendFiles", ["spaceId": id, "paths": ["/tmp/a.txt"]]) is [Any])
    }

    /// The operations the Electron router answers under the same names.
    @Test func theContractsOperationsAreRouted() {
        for m in ["spaces.cancelCreate", "host.status", "keyvault.approve", "keyvault.deny", "keyvault.revokeGrant",
                  "agents.list", "agents.runs", "agents.events", "agents.pause", "agents.resume",
                  "agents.setup", "agents.configure"] {
            #expect(WebUIBridge.methods.contains(m), "\(m)")
        }
    }

    @Test func approveDenyAndRevokeGoToTheBroker() async throws {
        let fake = FakeKeyvault(try fixtureOverview())
        let (bridge, model) = bridge(kv: fake)
        await model.keyvault.refresh()

        let grant = try await bridge.handle("keyvault.approve", ["requestId": "req-1", "items": NSNull()]) as? [String: Any]
        #expect(grant?["requestId"] as? String == "req-1")
        #expect(fake.commands.last == .approve(requestId: "req-1", items: nil))

        _ = try await bridge.handle("keyvault.deny", ["requestId": "req-2"])
        #expect(fake.commands.last == .deny(requestId: "req-2"))
        #expect(model.keyvault.overview.pending.isEmpty)
        // A request that is no longer waiting.
        #expect(await failure(bridge, "keyvault.approve", ["requestId": "req-1", "items": ["x"]]) == "not_found")
        #expect(await failure(bridge, "keyvault.deny", [:]) == "bad_args")

        let count = try await bridge.handle("keyvault.revokeGrant", ["id": "grant-1"]) as? NSNumber
        #expect(count?.intValue == 1)
        #expect(fake.commands.last == .revokeGrant(id: "grant-1"))
    }

    /// "No Keyvault yet" on the web page: Set up Keyvault runs the core's
    /// Touch ID setup and answers the recovery key once.
    @Test func setupFromTheWebPageAnswersTheRecoveryKey() async throws {
        var none = KeyvaultFixtures.overview()
        none.availability = "no_vault"
        none.status?.initialized = false
        none.status?.unlocked = false
        let fake = FakeKeyvault(none)
        let (bridge, model) = bridge(kv: fake)
        await model.keyvault.refresh()
        #expect(model.keyvault.page.canSetup)
        #expect(WebUIBridge.methods.contains("keyvault.setup"))

        let r = try await bridge.handle("keyvault.setup", [:]) as? [String: Any]
        #expect(r?["recoveryKey"] as? String == "WXYZ-2345")
        #expect(fake.commands.last == .setup)
        #expect(model.keyvault.overview.availability == "ready")
        // Set up already: nothing to set up.
        #expect(await failure(bridge, "keyvault.setup", [:]) == "failed")

    }

    /// A passphrase vault set up and unlocked from the web page: the native
    /// sheet fills the model's fields and the bridge submits them as the
    /// SwiftUI form does; the page's call carries no passphrase. Closing the
    /// sheet answers `cancelled` and clears the fields.
    @Test func passphraseSetupAndUnlockFromTheWebPageAskInTheSheet() async throws {
        let fake = FakeKeyvault(try keyvaultUnlockCase("setup-passphrase"))
        let (bridge, model) = bridge(kv: fake)
        let kv = model.keyvault
        await kv.refresh()
        #expect(kv.page.form?.method == .passphrase && kv.page.form?.mode == .setup)

        var asked: [KvCredentialForm] = []
        bridge.askPassphrase = { form in
            asked.append(form)
            kv.passphrase = "orbit lantern"
            return false
        }
        #expect(await failure(bridge, "keyvault.setup", [:]) == "cancelled")
        #expect(kv.passphrase.isEmpty && fake.passphrases.isEmpty)

        bridge.askPassphrase = { form in
            asked.append(form)
            kv.passphrase = "orbit lantern pickle harbor"
            kv.passphraseConfirm = "orbit lantern pickle harbor"
            return true
        }
        let r = try await bridge.handle("keyvault.setup", [:]) as? [String: Any]
        #expect(r?["recoveryKey"] as? String == "ABCDE-FGHJK")
        #expect(fake.passphrases == ["orbit lantern pickle harbor"])
        #expect(fake.commands.isEmpty)
        #expect(kv.passphrase.isEmpty && kv.passphraseConfirm.isEmpty)
        #expect(asked.allSatisfy { $0.mode == .setup && $0.confirmLabel != nil })

        fake.current = try keyvaultUnlockCase("unlock-passphrase")
        await kv.refresh()
        #expect(kv.page.canUnlock)
        bridge.askPassphrase = { form in
            asked.append(form)
            kv.passphrase = "orbit lantern pickle harbor"
            return true
        }
        _ = try await bridge.handle("keyvault.unlockVault", [:])
        #expect(asked.last?.mode == .unlock)
        #expect(fake.passphrases.count == 2)
        #expect(kv.page.ready)
    }

    @Test func cancelCreateAnswersWithTheSDKsWords() async throws {
        let (bridge, _) = bridge()
        let r = try await bridge.handle("spaces.cancelCreate", ["pendingId": "pending:nope"]) as? [String: Any]
        #expect(r?["state"] as? String == "not_creating")
        #expect(r?["id"] as? String == "pending:nope")
        #expect(await failure(bridge, "spaces.cancelCreate", [:]) == "bad_args")
    }

    @Test func hostStatusIsTheHostModelsState() async throws {
        let (bridge, _) = bridge(host: FixtureHost())
        let s = try await bridge.handle("host.status", [:]) as? [String: Any]
        #expect(s?["configured"] as? Bool == false)
        #expect(s?["serviceKind"] as? String == "process")
        #expect(s?["machineId"] is NSNull)
    }

    @Test func agentsWithoutTheDaemonAreUnsupported() async {
        let (bridge, _) = bridge()
        #expect(await failure(bridge, "agents.list", [:]) == "unsupported")
        #expect(await failure(bridge, "agents.events", ["spaceId": "s", "runId": "r", "cursor": 0]) == "unsupported")
        #expect(await failure(bridge, "agents.setup", [:]) == "unsupported")
        #expect(await failure(bridge, "agents.runs", [:]) == "bad_args")
        #expect(await failure(bridge, "agents.pause", [:]) == "bad_args")
    }

    @Test func agentRunsAndSetupRowsAreTheCoresRecords() async throws {
        let backend = FixtureSpacesBackend()
        backend.fixtureRuns["local:aurora"] = AgentRunsTests.runs
        let setup = FixtureAgentSetup()
        let (bridge, _) = bridge(backend: backend, agentSetup: setup)
        let runs = try await bridge.handle("agents.runs", ["spaceId": "local:aurora"]) as? [[String: Any]]
        #expect(runs?.map { $0["runId"] as? String } == ["r-fail", "r-run", "r-bad"])
        #expect(runs?[1]["status"] as? String == "running")
        let rows = try await bridge.handle("agents.setup", [:]) as? [[String: Any]]
        #expect(rows?.isEmpty == false)
        #expect(rows?.first?["skillsTotal"] != nil)
        let configured = try await bridge.handle("agents.configure", ["agents": ["claude-code"]]) as? [[String: Any]]
        #expect(configured?.first { $0["agent"] as? String == "claude-code" }?["configured"] as? Bool == true)
        // A Settings row's button, as the native one: Remove on a configured agent.
        _ = try await bridge.handle("settings.choose", ["row": "agent:claude-code", "option": "press"])
        let removed = try await bridge.handle("agents.setup", [:]) as? [[String: Any]]
        #expect(removed?.first { $0["agent"] as? String == "claude-code" }?["configured"] as? Bool == false)
    }

    /// The page opens one notification and marks just that one read; the
    /// list carries the Space each entry is about (its Open Space).
    @Test func oneNotificationIsMarkedReadAndCarriesItsSpace() async throws {
        let backend = BridgeContractTests.ToolBackend(FixtureSpacesBackend())
        backend.agents.notifications.append(["id": "n2", "at_ms": backend.agents.now - 30_000, "agent": "ada", "kind": "message",
                                             "title": "ada", "body": "Second.", "read": false])
        let (bridge, _) = bridge(backend: backend)
        func list() async throws -> [String: [String: Any]] {
            let rows = try await bridge.handle("notifications.list", [:]) as? [[String: Any]] ?? []
            return Dictionary(rows.map { ($0["id"] as? String ?? "", $0) }, uniquingKeysWith: { a, _ in a })
        }
        let before = try await list()
        #expect(before["n1"]?["space"] as? String == "local:dev")
        #expect(before["n2"]?["space"] is NSNull)
        #expect(before["n1"]?["body"] as? String == "Your research is ready.")
        #expect(before["n1"]?["read"] as? Bool == false)

        #expect(try await bridge.handle("notifications.markRead", ["ids": ["n1"]]) is NSNull)
        let after = try await list()
        #expect(after["n1"]?["read"] as? Bool == true)
        #expect(after["n2"]?["read"] as? Bool == false)

        // No ids marks nothing (the daemon would read it as every entry).
        _ = try await bridge.handle("notifications.markRead", ["ids": [String]()])
        #expect(try await list()["n2"]?["read"] as? Bool == false)
        #expect(await failure(bridge, "notifications.markRead", [:]) == "bad_args")
    }

    @Test func persistentAgentsListAsCamelCaseRecords() async throws {
        let m = PersistentModel(tools: FakeAgentsTools())
        let agents = try await m.listAgents()
        #expect(agents.map { $0["name"] as? String } == ["ada", "scout"])
        #expect(agents[1]["spaceState"] as? String == "released")
        #expect(m.agentCount == 2)
    }

    @Test func machinesCarryTheHostStatus() async throws {
        let (bridge, _) = bridge(host: FixtureHost())
        let m = try await bridge.handle("machines.list", [:]) as? [String: Any]
        #expect((m?["host"] as? [String: Any])?["serviceKind"] as? String == "process")
        // Who the relay sees connected, for machines the app's probe missed.
        #expect(m?["presence"] is [String: Bool])
        #expect(m?["deviceStates"] is [String: String])
    }

    @Test func theUpdateChannelIsReadAndChosenByRow() async throws {
        let (bridge, model) = bridge()
        #expect((try await bridge.handle("settings.get", [:]) as? [String: Any])?["updateChannel"] is NSNull)
        #expect(await failure(bridge, "settings.choose", ["row": "update-channel", "option": "beta"]) == "unsupported")
        model.updates = UpdatesModel(updater: FixtureUpdater(), channel: .stable)
        let s = try await bridge.handle("settings.choose", ["row": "update-channel", "option": "beta"]) as? [String: Any]
        #expect(s?["updateChannel"] as? String == "beta")
        #expect(model.updates.channel == .beta)
    }

    /// The page's core has no environment: the host names the account app
    /// Upgrade's billing fallback opens (CUA_ENV, CUA_ACCOUNT_APP_URL).
    @Test func settingsNameTheAccountApp() async throws {
        let (bridge, _) = bridge()
        let s = try await bridge.handle("settings.get", [:]) as? [String: Any]
        #expect(s?["accountUrl"] as? String == appAccountAppUrl())
    }

    @Test func theGlobalShortcutSaysWhetherTheSystemGaveIt() async throws {
        let (bridge, _) = bridge()
        defer { GlobalShortcut.installed = nil }
        GlobalShortcut.installed = nil
        #expect((try await bridge.handle("settings.get", [:]) as? [String: Any])?["globalShortcut"] is NSNull)
        // Another app holds the keys at launch, then lets them go: each read tries again.
        var answers = [false, true]
        var tries = 0
        GlobalShortcut.installed = GlobalShortcut { tries += 1; return answers.removeFirst() }
        GlobalShortcut.installed?.ensure()
        let refused = (try await bridge.handle("settings.get", [:]) as? [String: Any])?["globalShortcut"] as? [String: Any]
        #expect(refused?["label"] as? String == "\u{2318}\u{21e7}Space")
        #expect(refused?["registered"] as? Bool == true)
        #expect(tries == 2)
        _ = try await bridge.handle("settings.get", [:])
        #expect(tries == 2)
    }

    @Test func everyPageOperationIsRouted() {
        for m in ["spaces.add", "clouds.status", "teleport.catalog", "teleport.run", "sharing.share", "volume.overview",
                  "agents.setupDriver", "about.set", "loginItem.set", "devices.approve", "storage.run",
                  "notifications.list", "telemetry.track", "spaces.windows", "stream.pip", "spaces.thumbnail", "spaces.sendFiles", "host.setUp", "host.action",
                  "host.openSettings"] {
            #expect(WebUIBridge.methods.contains(m), "\(m)")
        }
        #expect(!WebUIBridge.methods.contains("spaces.new"))
    }

    /// New Space runs in the page on the env the native sheet opens with:
    /// it crosses the bridge and the core reads it back unchanged.
    @Test func createOptionsAreTheNativeSheetsEnv() async throws {
        let backend = FixtureSpacesBackend()
        backend.fixtureHosts = [AppSpaceHost(id: "96fedb7e", name: "gamma-4 Mac Studio", via: "relay", online: true,
                                             os: "macos", limits: [])]
        let (bridge, model) = bridge(backend: backend)
        let options = try #require(try await bridge.handle("spaces.createOptions", [:]) as? [String: Any])
        let env = try #require(options["env"] as? [String: Any])
        let json = String(decoding: try JSONSerialization.data(withJSONObject: env), as: UTF8.self)
        let decoded = try appWizardEnvFromJson(json: json)
        #expect(decoded == (await model.newSpaceEnv()))
        #expect(decoded.hosts.map(\.name) == ["gamma-4 Mac Studio"])
        #expect(!model.showingNewSpace)
    }

    /// Create runs the native sheet's create with the page's pending id:
    /// the pending row, the SDK's create, then the Space.
    @Test func createRunsTheAppsCreate() async throws {
        let backend = FixtureSpacesBackend()
        let (bridge, model) = bridge(backend: backend)
        let config: [String: Any] = ["image": "ghcr.io/trycua/linux:24.04", "on": "host:96fedb7e", "kind": "container",
                                     "runtime": "auto", "name": "demo", "cpus": 2, "memoryMb": 4096, "spacesd": true]
        let space = try await bridge.handle("spaces.create", ["config": config, "pendingId": "pending:web-1",
                                                               "os": "linux"]) as? [String: Any]
        #expect(space?["id"] as? String == "local:demo")
        #expect(backend.createIds == ["pending:web-1"])
        #expect(backend.created.first?.on == "host:96fedb7e")
        #expect(backend.created.first?.cpus == 2)
        #expect(model.creates.pending.isEmpty)
        #expect(await failure(bridge, "spaces.create", ["config": config, "pendingId": "nope"]) == "bad_args")
        #expect(await failure(bridge, "spaces.create", ["config": ["image": "x", "kind": "pod"], "pendingId": "pending:2"])
                == "bad_args")
    }

    @Test func aCancelledCreateAnswersCancelled() async throws {
        let backend = FixtureSpacesBackend()
        backend.cancelledCreates = ["pending:web-2"]
        let (bridge, _) = bridge(backend: backend)
        let config: [String: Any] = ["image": "ghcr.io/trycua/linux:24.04", "on": "local", "kind": "container"]
        #expect(await failure(bridge, "spaces.create", ["config": config, "pendingId": "pending:web-2"]) == "cancelled")
    }

    /// While New UI is on, New Space opens its wizard; the native sheet is
    /// the fallback.
    @Test func newSpaceOpensTheWebWizardWhileNewUIIsOn() async {
        let (_, model) = bridge()
        var asked: [String?] = []
        model.openWebNewSpace = { on in asked.append(on); return true }
        await model.openNewSpace()
        #expect(asked.isEmpty && model.showingNewSpace)
        model.cancelNewSpace()
        model.settings.experiments.webUi = true
        await model.openNewSpace()
        await model.openNewSpace(on: "host:m1")
        #expect(asked == [nil, "host:m1"])
        #expect(!model.showingNewSpace)
    }

    @Test func usageEventsRespectTheSwitchAndCarryOnlyFixedWords() async throws {
        let (bridge, model) = bridge()
        let telemetry = try #require(model.telemetrySink as? FixtureTelemetry)
        let before = telemetry.recorded.count
        let signals: [Any] = [["type": "feature", "feature": "space_open"],
                              ["type": "feature", "feature": "/Users/ada"],
                              ["type": "step", "step": "signed_in", "ok": "yes"],
                              ["type": "nope"],
                              ["type": "space-create", "location": "local", "guestOs": "linux", "kind": "vm",
                               "outcome": "ready", "failedPhase": "none", "stalled": false, "elapsedMs": 10, "gpu": false]]
        _ = try await bridge.handle("telemetry.track", ["signals": signals])
        #expect(Array(telemetry.recorded.dropFirst(before)) == [.feature(feature: "space_open")])
        _ = try telemetry.setEnabled(false)
        _ = try await bridge.handle("telemetry.track", ["signals": signals])
        #expect(telemetry.recorded.count == before + 1)
    }

    @Test func pagesWithoutTheirServiceSayUnsupported() async {
        let (bridge, _) = bridge()
        #expect(await failure(bridge, "volume.overview", [:]) == "unsupported")
        #expect(await failure(bridge, "notifications.list", [:]) == "unsupported")
        #expect(await failure(bridge, "notifications.markRead", ["ids": ["n1"]]) == "unsupported")
        #expect(await failure(bridge, "about.set", ["autoCheck": true]) == "unsupported")
        #expect(await failure(bridge, "host.openSettings", ["url": "https://example.com"]) == "bad_args")
        #expect(await failure(bridge, "volume.reveal", ["path": "/etc/hosts"]) == "bad_args")
    }

    @Test func hostActionsRunThroughTheHostModel() async throws {
        let host = FixtureHost()
        let (bridge, _) = bridge(host: host)
        _ = try await bridge.handle("host.status", [:])
        let s = try await bridge.handle("host.action", ["action": "provide-spaces"]) as? [String: Any]
        #expect(s?["provideSpaces"] as? Bool == true)
        #expect(await failure(bridge, "host.action", ["action": "set-up"]) == "bad_args")
    }

    @Test func hostStatusCarriesTheWholeStateAndTheProgress() async throws {
        let host = FixtureHost()
        let (bridge, _) = bridge(host: host)
        _ = try await bridge.handle("host.setUp", ["request": ["mode": "relay", "name": "Studio"]])
        let s = try #require(try await bridge.handle("host.status", [:]) as? [String: Any])
        // What the core's panel reads beyond the first fields: the logs,
        // the limits, the owner, sharing paused while signed out.
        #expect(s["maxMacosVms"] as? Int == 2)
        #expect(s["recentAccess"] is [Any])
        #expect(s["providedSpaces"] is [Any])
        #expect(s["spacesAudit"] is [Any])
        #expect(s["pausedSignedOut"] as? Bool == false)
        #expect(s["owner"] as? String == "user-1")
        #expect(s["ownerEmail"] as? String == "ada@example.com")
        #expect(s.keys.contains("account"))
        #expect(s["progress"] is NSNull)
    }

    @Test func aFailedSetUpIsWordedAsTheNativeFormWordsIt() async throws {
        let host = FixtureHost()
        host.failSetup = "error sending request for url (https://relay.cua.ai): connection refused"
        let (bridge, _) = bridge(host: host)
        do {
            _ = try await bridge.handle("host.setUp", ["request": ["mode": "direct", "direct": "0.0.0.0:3211"]])
            Issue.record("expected a failure")
        } catch let f as WebUIBridge.Failure {
            #expect(f.code == "failed")
            #expect(f.presented?.kind == .network)
            #expect(f.message == f.presented?.message)
            let e = try #require(WebUIBridge.reply(id: "1", error: f)["error"] as? [String: Any])
            #expect(e["title"] as? String == "Couldn\u{2019}t connect")
            #expect(e["message"] as? String == "Cua Spaces couldn\u{2019}t reach the internet. Check your connection and try again.")
            #expect((e["details"] as? String)?.contains("connection refused") == true)
            #expect(e["actionLabel"] as? String == "Retry")
        }
    }

    @Test func aFailedButtonIsWordedWithTheRawErrorAsDetails() async throws {
        let host = FixtureHost()
        let (bridge, _) = bridge(host: host)
        _ = try await bridge.handle("host.setUp", ["request": ["mode": "direct", "direct": "0.0.0.0:3211"]])
        host.failConfigure = "launchctl bootstrap failed: 5"
        do {
            _ = try await bridge.handle("host.action", ["action": "provide-spaces"])
            Issue.record("expected a failure")
        } catch let f as WebUIBridge.Failure {
            #expect(f.presented?.kind == .service)
            #expect(f.presented?.details.contains("launchctl bootstrap failed") == true)
            #expect(f.message == f.presented?.message)
        }
        // Errors with no words of their own keep the plain envelope.
        let plain = WebUIBridge.reply(id: "2", error: .badArgs("nope"))["error"] as? [String: Any]
        #expect(plain?["title"] == nil)
    }

    @Test func signInIsAHostAction() async throws {
        let (bridge, _) = bridge(host: FixtureHost())
        #expect(try bridge.hostAction("sign-in") == .signIn)
        _ = try await bridge.handle("host.status", [:])
        // No account wired: nothing to sign in to, and the status answers.
        let s = try await bridge.handle("host.action", ["action": "sign-in"]) as? [String: Any]
        #expect(s?["configured"] as? Bool == false)
    }

    @Test func devicesShowEnrollmentWhenTheRelayRefusesThisDevice() async throws {
        let relay = FixtureDevices()
        relay.failSnapshot = "relay: this device is not enrolled for your cua.ai account"
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("cua-webui-\(UUID().uuidString)")
        let model = AppModel(backend: FixtureSpacesBackend(), keyvault: KeyvaultModel(client: nil),
                             onboarding: OnboardingModel(statePath: dir.appendingPathComponent("onboarding.json").path),
                             settingsPath: dir.appendingPathComponent("settings.json").path,
                             telemetry: FixtureTelemetry(), devices: relay)
        model.devices.signedIn = true
        let bridge = WebUIBridge(model: model, allowedOrigins: [])
        // The page still draws (this device "Needs enrollment", Enroll…),
        // with the relay's words under it, as Settings → Devices does.
        let input = try #require(try await bridge.handle("devices.get", [:]) as? [String: Any])
        #expect((input["readError"] as? String)?.contains("not enrolled") == true)
        #expect((input["devices"] as? [Any])?.isEmpty == true)
        #expect(input["deviceName"] as? String == Foundation.Host.current().localizedName)
        #expect(model.devices.view.thisDevice.actionLabel != nil)
        // Enroll… then works from there.
        let r = try #require(try await bridge.handle("devices.enroll", [:]) as? [String: Any])
        #expect(r["code"] as? String == "K7QX-M2RP")
        relay.failSnapshot = nil
        let read = try #require(try await bridge.handle("devices.get", [:]) as? [String: Any])
        #expect(read["readError"] is NSNull)
    }

    @Test func checkNowAnswersBeforeSparklesAlert() async throws {
        let (bridge, model) = bridge()
        let updater = FixtureUpdater()
        model.updates = UpdatesModel(updater: updater, channel: .stable)
        let a = try #require(try await bridge.handle("about.checkNow", [:]) as? [String: Any])
        // The page has its answer before the check (and its modal alert) runs.
        #expect(a["checking"] as? Bool == true)
        #expect(updater.checks == 0)
        await withCheckedContinuation { k in DispatchQueue.main.async { k.resume() } }
        #expect(updater.checks == 1)
    }

    @Test func showWelcomeAgainRestartsTheFirstRun() async throws {
        let (bridge, model) = bridge()
        model.onboarding.finish()
        #expect(model.onboarding.completed)
        _ = try await bridge.handle("settings.choose", ["row": "welcome", "option": "show"])
        #expect(!model.onboarding.completed)
    }

    @Test func turningNewUIOffClosesItsWindow() {
        let (_, model) = bridge()
        var closed = 0
        model.closeWebUI = { closed += 1 }
        model.chooseExperiment(row: "experiment:web_ui", option: "on")
        #expect(closed == 0)
        model.chooseExperiment(row: "experiment:web_ui", option: "off")
        #expect(closed == 1)
    }

    @Test func theNewUIWindowComesBackWhereItWas() {
        _ = NSApplication.shared
        let (_, model) = bridge()
        UserDefaults.standard.removeObject(forKey: "NSWindow Frame \(WebUIWindowController.frameName)")
        let first = WebUIWindowController(model: model)
        let frame = NSRect(x: 140, y: 120, width: 1010, height: 640)
        first.window?.setFrame(frame, display: false)
        first.window?.close()
        let second = WebUIWindowController(model: model)
        defer { second.window?.close() }
        #expect(second.window?.frame.size == frame.size)
        #expect(second.window?.frame.origin == frame.origin)
    }

    /// A context menu from the page: its items in an NSMenu, in order, with
    /// their enabled state; the chosen id answers the page, and a chosen
    /// item's `copy` goes on the pasteboard. A bad request is refused, and
    /// without a window the page is told to draw its own.
    @Test func thePagesContextMenusAreNative() async throws {
        _ = NSApplication.shared
        let (_, model) = bridge()
        let c = WebUIWindowController(model: model)
        defer { c.window?.close() }
        let board = NSPasteboard(name: .init("cua-test-menu-\(UUID().uuidString)"))
        c.bridge.menuPasteboard = { board }
        var shown: [([WebUIMenuItem], CGPoint)] = []
        c.bridge.pickMenuItem = { items, at in
            shown.append((items, at))
            return "space.copyId"
        }
        let args: [String: Any] = ["items": [["id": "space.open", "label": "Open"], ["id": "", "label": "", "separator": true],
                                             ["id": "space.stop", "label": "Turn off…", "enabled": false, "destructive": true],
                                             ["id": "space.copyId", "label": "Copy ID", "copy": "local:aurora"]],
                                   "x": 40, "y": 80]
        let answer = try await c.bridge.handle("window.showContextMenu", args) as? [String: Any]
        #expect(answer?["id"] as? String == "space.copyId")
        #expect(board.string(forType: .string) == "local:aurora")
        #expect(shown.first?.1 == CGPoint(x: 40, y: 80))

        let picker = WebUIMenuPicker()
        let menu = WebUIWindowController.contextMenu(try #require(shown.first?.0), picker: picker)
        #expect(menu.items.map { $0.isSeparatorItem ? "-" : $0.title } == ["Open", "-", "Turn off…", "Copy ID"])
        #expect(menu.items.filter { !$0.isSeparatorItem }.map(\.isEnabled) == [true, false, true])
        menu.performActionForItem(at: 0)
        #expect(picker.chosen == "space.open")

        c.bridge.pickMenuItem = { _, _ in nil }
        let none = try await c.bridge.handle("window.showContextMenu", args) as? [String: Any]
        #expect(none?["id"] is NSNull)

        c.webView.pageZoom = 1.25
        let p = c.viewPoint(CGPoint(x: 40, y: 80))
        #expect(p.x == 50)
        #expect(p.y == (c.webView.isFlipped ? 100 : c.webView.bounds.height - 100))

        await #expect(throws: (any Error).self) {
            _ = try await c.bridge.handle("window.showContextMenu", ["items": [["id": "a"]], "x": 1, "y": 1])
        }
        await #expect(throws: (any Error).self) {
            _ = try await c.bridge.handle("window.showContextMenu", ["items": [["id": "a", "label": "A"]], "x": "1", "y": 1])
        }
        // No window to draw over: unimplemented, and the page draws its own.
        let (bare, _) = bridge()
        #expect(await failure(bare, "window.showContextMenu", args) == "unimplemented")
    }

    /// The page's Space tiles become the window-drag targets of its window
    /// (CSS pixels at the page's zoom), and a targeted tile is named back to
    /// the page; a new page starts with none.
    @Test func thePagesTilesAreWindowDropTargets() async throws {
        _ = NSApplication.shared
        let (_, model) = bridge()
        let c = WebUIWindowController(model: model)
        defer { c.window?.close() }
        var emitted: [(String, Any?)] = []
        c.onEmit = { emitted.append(($0, $1)) }
        _ = try await c.bridge.handle("window.setDropTargets",
                                      ["targets": [["spaceId": "local:aurora", "x": 20, "y": 60, "width": 232, "height": 180]]])
        let targets = model.pageDropTargets
        #expect(targets.window === c.window)
        #expect(targets.frames == ["local:aurora": CGRect(x: 20, y: 60, width: 232, height: 180)])
        c.webView.pageZoom = 1.25
        _ = try await c.bridge.handle("window.setDropTargets",
                                      ["targets": [["spaceId": "local:aurora", "x": 20, "y": 60, "width": 232, "height": 180]]])
        #expect(targets.frames["local:aurora"] == CGRect(x: 25, y: 75, width: 290, height: 225))
        await #expect(throws: (any Error).self) {
            _ = try await c.bridge.handle("window.setDropTargets", ["targets": [["x": 1]]])
        }
        targets.handle(phase: "move", target: "local:aurora")
        targets.handle(phase: "move", target: "local:aurora")
        targets.handle(phase: "end", target: "local:aurora")
        let named = emitted.filter { $0.0 == "window.dropTarget" }.map { ($0.1 as? [String: Any])?["spaceId"] }
        #expect(named.count == 2)
        #expect(named.first as? String == "local:aurora")
        #expect(named.last as? NSNull != nil)
        c.window?.close()
        #expect(targets.frames.isEmpty)
    }

    @Test func aboutIsTheUpdatesModelsInput() async throws {
        let (bridge, model) = bridge()
        model.updates = UpdatesModel(updater: FixtureUpdater(), channel: .stable)
        let a = try await bridge.handle("about.set", ["autoCheck": true, "channel": "beta"]) as? [String: Any]
        #expect(a?["autoCheck"] as? Bool == true)
        #expect(a?["channel"] as? String == "beta")
        #expect(a?["updater"] as? Bool == true)
    }
}
