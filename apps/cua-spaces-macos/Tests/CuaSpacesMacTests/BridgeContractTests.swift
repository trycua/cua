// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesMacKit
import AppKit
import CuaSDK
import CuaSpacesStreaming
import CuaSpacesFFI
import Foundation
import Testing

/// The web UI's bridge contract on the Mac (`apps/cua-spaces-web/src/bridge`):
///
/// - every method `WebUIBridge.methods` lists is routed (it never answers
///   `unimplemented`), and every method the routing switches name is listed
///   (the web side checks the list against `WEBKIT_METHODS` and `coverage.ts`);
/// - the answers have the shapes the page reads: each one, on fixture
///   backends, is checked against `contracts/bridge-shapes.json` (exported
///   from `contracts/shapes.ts`; `pnpm contract:shapes` there rewrites it).
///
/// `CUA_BRIDGE_ANSWERS=<path>` also writes every answer there, as JSON
/// (for a look at what the page gets).
@Suite("Bridge contract")
@MainActor
struct BridgeContractTests {
    static let webBridge = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        .appendingPathComponent("../cua-spaces-web/src/bridge").standardizedFileURL
    static let sources = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        .appendingPathComponent("Sources/CuaSpacesMacKit/WebHost")

    init() { _ = NSApplication.shared }

    // MARK: - Routing

    /// A bare model: nothing signed in, no daemon, no updater. Every method
    /// still has a route; what it can't do here fails with its own code.
    func bareBridge() -> WebUIBridge {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("cua-contract-\(UUID().uuidString)")
        let model = AppModel(backend: FixtureSpacesBackend(), keyvault: KeyvaultModel(client: nil),
                             onboarding: OnboardingModel(statePath: dir.appendingPathComponent("onboarding.json").path),
                             settingsPath: dir.appendingPathComponent("settings.json").path,
                             telemetry: FixtureTelemetry())
        let bridge = WebUIBridge(model: model, allowedOrigins: [])
        bridge.actions = WebUIActions(openSpace: { _ in }, openMain: {})
        // "Send file…" picks this instead of showing the native panel.
        bridge.pickFiles = { [URL(fileURLWithPath: "/tmp/cua-contract-notes.txt")] }
        return bridge
    }

    func code(_ bridge: WebUIBridge, _ method: String, _ args: [String: Any] = [:]) async -> String? {
        do {
            _ = try await bridge.handle(method, args)
            return nil
        } catch let f as WebUIBridge.Failure {
            return f.code
        } catch {
            return "failed"
        }
    }

    @Test func everyListedMethodIsRouted() async {
        let bridge = bareBridge()
        var unrouted: [String] = []
        for method in WebUIBridge.methods where await code(bridge, method) == "unimplemented" {
            unrouted.append(method)
        }
        #expect(unrouted == [], "listed in WebUIBridge.methods, answered unimplemented")
        #expect(await code(bridge, "spaces.nope") == "unimplemented")
        #expect(Set(WebUIBridge.methods).count == WebUIBridge.methods.count, "listed twice")
    }

    /// Every `case "a.b"` in the routing switches is a listed method (the
    /// sub-switches' words have no dot).
    @Test func everyRoutedMethodIsListed() throws {
        let files = try FileManager.default.contentsOfDirectory(at: Self.sources, includingPropertiesForKeys: nil)
            .filter { $0.lastPathComponent.hasPrefix("WebUIBridge") && $0.pathExtension == "swift" }
        #expect(files.count >= 3)
        let caseLine = try NSRegularExpression(pattern: #"^\s*case\s+("[^"]*"(?:\s*,\s*"[^"]*")*)\s*:"#,
                                               options: [.anchorsMatchLines])
        let quoted = try NSRegularExpression(pattern: #""([^"]+)""#)
        var routed = Set<String>()
        for file in files {
            // Continuation lines of a multi-line `case` list too.
            let src = try String(contentsOf: file, encoding: .utf8)
                .replacingOccurrences(of: #",\s*\n\s*""#, with: #", ""#, options: .regularExpression)
            let ns = src as NSString
            for m in caseLine.matches(in: src, range: NSRange(location: 0, length: ns.length)) {
                let list = ns.substring(with: m.range(at: 1))
                for q in quoted.matches(in: list, range: NSRange(location: 0, length: (list as NSString).length)) {
                    let word = (list as NSString).substring(with: q.range(at: 1))
                    if word.contains(".") { routed.insert(word) }
                }
            }
        }
        #expect(routed.count > 50)
        #expect(routed.subtracting(WebUIBridge.methods).sorted() == [], "routed but not in WebUIBridge.methods")
        #expect(Set(WebUIBridge.methods).subtracting(routed).sorted() == [], "listed but no case names it")
    }

    // MARK: - Shapes

    /// A fixture backend with the daemon's tools: the Agents, Volume and
    /// agent keys fakes, by tool name.
    final class ToolBackend: SpacesBackend, AgentsToolRunning, CloudToolRunning, @unchecked Sendable {
        let inner: FixtureSpacesBackend
        let agents = FakeAgentsTools()
        let drive = FakeDriveTools()
        let keys = FakeAgentKeysTools()
        init(_ inner: FixtureSpacesBackend) { self.inner = inner }

        func agentsTool(_ tool: String, _ args: [String: Any]) async throws -> Any {
            if tool.hasPrefix("agent_keys.") { return try await keys.agentsTool(tool, args) }
            if tool.hasPrefix("volume_") && !["volume_requests", "volume_grants"].contains(tool) {
                return try await drive.agentsTool(tool, args)
            }
            return try await agents.agentsTool(tool, args)
        }
        func cloudTool(_ tool: String, _ args: [String: Any]) async throws -> Any { try await inner.cloudTool(tool, args) }

        func rows() async throws -> [AppSpaceRow] { try await inner.rows() }
        func create(_ args: AppCreateSpaceArgs, createId: String,
                    progress: @escaping @Sendable (SpaceCreateProgress) -> Void) async throws -> String {
            try await inner.create(args, createId: createId, progress: progress)
        }
        func cancelCreate(createId: String) async throws { try await inner.cancelCreate(createId: createId) }
        func gpuChoices() async -> [AppGpuChoice]? { await inner.gpuChoices() }
        func hosts() async -> [AppSpaceHost] { await inner.hosts() }
        func reportedHostname(id: String) -> String? { inner.reportedHostname(id: id) }
        func add(url: String, token: String?, name: String?) async throws { try await inner.add(url: url, token: token, name: name) }
        func remove(id: String, removeOnly: Bool) async throws { try await inner.remove(id: id, removeOnly: removeOnly) }
        func setPower(id: String, on: Bool) async throws { try await inner.setPower(id: id, on: on) }
        func streamProvider(id: String) async throws -> SpaceStreamSourceProviding { try await inner.streamProvider(id: id) }
        func localBackends() async -> [String]? { await inner.localBackends() }
        func localRuntimes() async -> (ready: [String], details: [String: String])? { await inner.localRuntimes() }
        func lumeSource() async -> String? { await inner.lumeSource() }
        func setLumeSource(_ value: String) async throws { try await inner.setLumeSource(value) }
        func linuxSource() async -> String? { await inner.linuxSource() }
        func setLinuxSource(_ value: String) async throws { try await inner.setLinuxSource(value) }
        func localStorage() async -> LocalStorage? { await inner.localStorage() }
        func cloudAvailable() async -> Bool { await inner.cloudAvailable() }
        func runningMacosVms() async -> Int? { await inner.runningMacosVms() }
        func cloudPricing() async -> AppCloudPricing? { await inner.cloudPricing() }
        func teleportContext(id: String) async throws -> (CuaSpacesFFI.Teleport, CuaSDK.Space)? {
            try await inner.teleportContext(id: id)
        }
        func teleportHandle() -> CuaSpacesFFI.Teleport? { inner.teleportHandle() }
        func sendFiles(id: String, paths: [String]) async throws -> [AppSentFileInfo] {
            try await inner.sendFiles(id: id, paths: paths)
        }
        func agentRuns(id: String) async throws -> [AppSpaceAgentRun] { try await inner.agentRuns(id: id) }
        func thumbnail(id: String, maxAgeMs: UInt64?) async -> SpaceThumbnailData? {
            await inner.thumbnail(id: id, maxAgeMs: maxAgeMs)
        }
        func appIcons(id: String, requests: [SpaceAppIconRequest]) async -> [Data?] {
            await inner.appIcons(id: id, requests: requests)
        }
        func primaryDisplay(id: String) async -> AppStreamDisplay? { await inner.primaryDisplay(id: id) }
        func usage(id: String) async -> AppSpaceUsage? { await inner.usage(id: id) }
        func shares(id: String) async throws -> [AppShareEntryInput] { try await inner.shares(id: id) }
        func share(id: String, who: String, role: String) async throws -> [AppShareEntryInput] {
            try await inner.share(id: id, who: who, role: role)
        }
        func unshare(id: String, who: String) async throws -> [AppShareEntryInput] {
            try await inner.unshare(id: id, who: who)
        }
    }

    /// The app as `CUA_SPACES_FIXTURES=1` runs it, plus the daemon's tools,
    /// a Keyvault broker and an updater.
    func fixtureBridge(startup: StartupModel? = nil) async throws -> (WebUIBridge, AppModel, String) {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("cua-contract-\(UUID().uuidString)")
        let inner = FixtureSpacesBackend()
        inner.fixtureRuns["local:aurora"] = AgentRunsTests.runs
        let model = AppModel(backend: ToolBackend(inner),
                             keyvault: KeyvaultModel(client: FakeKeyvault(try fixtureOverview()), clock: { fixtureNow }),
                             onboarding: OnboardingModel(statePath: dir.appendingPathComponent("onboarding.json").path),
                             settingsPath: dir.appendingPathComponent("settings.json").path,
                             host: FixtureHost(), account: FixtureAccount(identity: "ada@example.com"),
                             telemetry: FixtureTelemetry(), agentSetup: FixtureAgentSetup(),
                             billing: FixtureBilling(), devices: FixtureDevices(), presence: FixturePresence(),
                             loginItem: FixtureLoginItem(.enabled), startup: startup)
        model.updates = UpdatesModel(updater: FixtureUpdater(lastCheck: fixtureNow), channel: .stable)
        await model.refresh()
        await model.keyvault.refresh()
        let bridge = WebUIBridge(model: model, allowedOrigins: [])
        bridge.actions = WebUIActions(openSpace: { _ in }, openMain: {})
        // "Send file…" picks this instead of showing the native panel.
        bridge.pickFiles = { [URL(fileURLWithPath: "/tmp/cua-contract-notes.txt")] }
        let space = try #require(model.spaces.first?.id)
        return (bridge, model, space)
    }

    /// Each method with arguments that take it down its usual path on the
    /// fixtures. A method left out says why in `skipped`.
    func cases(space: String) -> [(String, [String: Any])] {
        [
            ("app.info", [:]), ("session.get", [:]), ("session.signOut", [:]),
            ("spaces.list", [:]),
            ("spaces.createOptions", [:]), ("spaces.cancelCreate", ["pendingId": "pending:none"]),
            ("spaces.setPower", ["id": space, "on": true]),
            ("spaces.open", ["id": space]),
            ("machines.list", [:]), ("host.status", [:]),
            ("agents.list", [:]), ("agents.runs", ["spaceId": "local:aurora"]), ("agents.setup", [:]),
            ("agents.configure", ["agents": ["claude-code"]]),
            ("agentKeys.list", [:]), ("agentKeys.set", ["provider": "anthropic", "value": "sk-ant-test-0000"]),
            ("agentKeys.remove", ["env": "ANTHROPIC_API_KEY"]),
            ("clouds.status", [:]), ("clouds.test", ["target": ["provider": "aws", "region": "us-west-2"]]),
            ("clouds.connect", ["target": ["provider": "aws", "region": "us-west-2"], "makeDefault": false]),
            ("sharing.list", ["spaceId": space]), ("sharing.share", ["spaceId": space, "who": "bob@example.com", "role": "viewer"]),
            ("sharing.unshare", ["spaceId": space, "who": "bob@example.com"]),
            ("volume.overview", [:]), ("volume.storage", [:]), ("volume.mount", [:]), ("volume.unmount", [:]),
            ("volume.storageSet", ["update": ["backend": "fs", "dry_run": true]]),
            ("about.get", [:]), ("about.set", ["channel": "beta"]), ("about.checkNow", [:]),
            ("loginItem.get", [:]), ("loginItem.set", ["on": true]),
            ("devices.get", [:]), ("devices.enroll", [:]), ("devices.checkEnrolled", [:]),
            ("storage.get", [:]), ("storage.run", ["request": ["kind": "clear-cache"]]),
            ("notifications.list", [:]), ("notifications.markAllRead", [:]),
            ("telemetry.track", ["signals": [["type": "step", "step": "app_launched", "ok": true]]]),
            ("spaces.usage", ["spaceId": space]), ("spaces.windows", ["spaceId": space]),
            ("spaces.thumbnail", ["spaceId": space]),
            ("spaces.droppedFiles", ["names": ["notes.txt"]]), ("spaces.chooseFiles", [:]),
            ("spaces.sendFiles", ["spaceId": space, "paths": ["/tmp/cua-contract-notes.txt"]]),
            ("settings.get", [:]), ("settings.choose", ["row": "telemetry", "option": "off"]),
            ("keyvault.get", [:]),
            ("keyvault.lock", ["ids": ["kv-1"]]),
            ("keyvault.approve", ["requestId": "req-1", "items": NSNull()]), ("keyvault.deny", ["requestId": "req-2"]),
            ("keyvault.revokeGrant", ["id": "grant-1"]), ("keyvault.setDisabled", ["disabled": false]),
            ("keyvault.showItems", [:]), ("keyvault.dismiss", ["imports": ["imp-1"]]),
            ("keyvault.run", ["command": ["type": "revoke-grant", "id": "grant-1"]]),
            ("teleport.remembered", ["providerId": "chrome", "spaceId": space]),
            ("window.setBackgroundColor", ["color": "#16181c"]),
            ("window.setDragRegions", ["rects": [["x": 78, "y": 0, "width": 400, "height": 38]]]),
            ("startup.get", [:]),
        ]
    }

    /// Methods not called here, and why (the rest of `WebUIBridge.methods`
    /// must be in `cases`).
    static let skipped: [String: String] = [
        "session.signIn": "starts a device sign-in that waits for the browser",
        "spaces.create": "runs a create to the end; CreateOptionsTests and ViewModelTests cover it",
        "spaces.delete": "removes the fixture Space the later cases use",
        "spaces.add": "needs a running cua-spacesd at the address",
        "host.setUp": "installs the host service", "host.action": "changes the host service",
        "host.openSettings": "opens System Settings",
        "agents.events": "needs the daemon's agent_events; WebUIHostTests checks it is unsupported without it",
        "agents.pause": "changes the fake's state for agents.list", "agents.resume": "as agents.pause",
        "agents.setupDriver": "writes the coding agents' configs on the fixture setup",
        // Teleport needs the SDK's handle and a reachable Space; its answers
        // are checked from the records the routes encode (teleportAnswers).
        "teleport.catalog": "needs the SDK's teleport handle", "teleport.entryForPath": "needs the SDK's teleport handle",
        "teleport.windows": "lists this Mac's real windows", "teleport.remoteWindows": "needs a Space's stream",
        "teleport.icon": "needs the SDK's teleport handle", "teleport.thumbnail": "needs the SDK's teleport handle",
        "teleport.plan": "needs the SDK's teleport handle", "teleport.run": "needs the SDK's teleport handle",
        "teleport.sites": "reads a browser's profile through the Keyvault", "teleport.streamWindow": "opens a native window",
        "volume.approve": "the fake has no request", "volume.deny": "the fake has no request",
        "volume.revoke": "the fake has no grant", "volume.resolve": "changes the fake's conflicts",
        "volume.reveal": "opens Finder", "loginItem.openSettings": "opens System Settings",
        "devices.approve": "asks for presence", "devices.rename": "changes the fixture account",
        "devices.revoke": "changes the fixture account", "devices.confirmMachine": "changes the fixture account",
        "stream.pip": "opens a picture-in-picture window",
        "keyvault.delete": "asks in an alert; TeleportBridgeTests answers it",
        "keyvault.unlock": "asks for Touch ID", "keyvault.unlockVault": "asks for Touch ID",
        "keyvault.setup": "sets the vault up with Touch ID; keyvaultSetupRefusesASetUpVault covers the fixture",
        "startup.act": "brings the app to the front; startupStatesHaveTheirShape covers the states",
    ]

    static func shapes() throws -> [String: Any] {
        let url = webBridge.appendingPathComponent("contracts/bridge-shapes.json")
        let data = try Data(contentsOf: url)
        return try #require(try JSONSerialization.jsonObject(with: data) as? [String: Any])
    }

    /// What WebKit hands the page: the answer as JSON.
    static func json(_ value: Any) throws -> Any {
        let data = try JSONSerialization.data(withJSONObject: ["v": value], options: [.fragmentsAllowed])
        return try #require((try JSONSerialization.jsonObject(with: data) as? [String: Any])?["v"])
    }

    @Test func everyMethodHasACaseOrAReason() {
        let named = Set(cases(space: "s").map(\.0)).union(Self.skipped.keys)
        #expect(Set(WebUIBridge.methods).subtracting(named).sorted() == [], "add a case or a reason")
        #expect(named.subtracting(WebUIBridge.methods).sorted() == [], "stale cases")
    }

    @Test func answersHaveTheShapesThePageReads() async throws {
        let doc = try Self.shapes()
        let webkit = try #require(doc["webkit"] as? [String: Any])
        let defs = doc["$defs"] as? [String: Any] ?? [:]
        let (bridge, _, space) = try await fixtureBridge()
        var problems: [String] = []
        var answers: [String: Any] = [:]
        for (method, args) in cases(space: space) {
            let answer: Any
            do {
                answer = try Self.json(try await bridge.handle(method, args))
            } catch {
                problems.append("\(method) failed: \(error)")
                continue
            }
            answers[method] = answer
            guard let schema = webkit[method] else {
                problems.append("\(method): no shape in bridge-shapes.json")
                continue
            }
            problems += JSONShape.validate(schema, answer, defs: defs).map { "\(method) \($0)" }
        }
        for (method, value) in try Self.teleportAnswers() {
            answers[method] = value
            guard let schema = webkit[method] else {
                problems.append("\(method): no shape in bridge-shapes.json")
                continue
            }
            problems += JSONShape.validate(schema, value, defs: defs).map { "\(method) \($0)" }
        }
        if let out = ProcessInfo.processInfo.environment["CUA_BRIDGE_ANSWERS"] {
            let data = try JSONSerialization.data(withJSONObject: answers, options: [.prettyPrinted, .sortedKeys])
            try data.write(to: URL(fileURLWithPath: out))
        }
        #expect(problems == [])
    }

    /// What the teleport routes answer, from the records they encode (the
    /// SDK's handle and a reachable Space have no fixture): a catalog entry,
    /// windows, an icon, a plan, a run report and a site inventory.
    static func teleportAnswers() throws -> [(String, Any)] {
        let entry = AppCatalogEntry(
            id: "slack", name: "Slack", hostPath: "/Applications/Slack.app", hostAppId: "com.tinyspeck.slackmacgap",
            version: "4.41", capability: .full, reason: nil, moves: [.appOnly, .appWithState], providerId: "slack",
            sensitiveGroups: [.signIns], installSource: "manifest", installId: "slack", installVersion: nil,
            launchBin: nil, lastUsedMs: 1_700_000_000_000, json: "{}")
        let plan = AppTeleportPlan(
            app: entry, spaceId: "local:aurora", moves: .appWithState,
            steps: [AppPlanStepView(kind: "install", summary: "Install Slack"), AppPlanStepView(kind: "state", summary: "Sign-ins")],
            consent: [AppConsentItem(kind: .state, key: "cookies", label: "Sign-ins", detail: "3 sites", bytes: 2048, sensitive: true)],
            sensitive: true, totalBytes: 2048, warnings: ["Slack must be closed"], relayUnsealed: false, json: "plan-1")
        let report = TeleportRunReport(appId: "slack", installed: ["slack"], sent: [], imported: ["cookies"],
                                       skipped: [], launched: true)
        let sites = KvInventory(providerId: "chrome", appDisplay: "Google Chrome", domains: [
            KvDomainCount(domain: "slack.com", cookies: 4, sessionCookies: 1, localStorage: 2, passwords: 0,
                          signin: true, identityProvider: false, unavailable: 0, unavailableReason: ""),
        ], notes: [])
        let open = AppOpenWindow(windowId: 41, appId: "", appName: "Slack", windowTitle: "general", supported: true,
                                 bundlePath: "/Applications/Slack.app")
        let remote = AppRemoteWindow(id: "w-1", appName: "Firefox", title: "Mozilla Firefox", visible: true, appId: "firefox",
                                     targetEpoch: 3, widthPx: 1280, heightPx: nil, pid: nil)
        let png = WebUIBridge.dataURL(Data([0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A]))
        return try [
            ("teleport.catalog", [BridgeValue.encode(entry)]),
            ("teleport.entryForPath", BridgeValue.encode(entry)),
            ("teleport.windows", BridgeValue.encode([open])),
            ("teleport.remoteWindows", BridgeValue.encode([remote])),
            ("teleport.icon", png),
            ("teleport.thumbnail", WebUIBridge.dataURL(nil)),
            ("teleport.plan", BridgeValue.encode(plan)),
            ("teleport.run", WebUIBridge.runReport(report)),
            ("teleport.sites", BridgeValue.encode(sites)),
            ("teleport.streamWindow", NSNull()),
        ].map { ($0.0, try json($0.1)) }
    }

    /// Set up Keyvault on a vault that is set up already: refused, with
    /// no Touch ID asked (the real setup is the native Keyvault's).
    @Test func keyvaultSetupRefusesASetUpVault() async throws {
        let (bridge, _, _) = try await fixtureBridge()
        #expect(await code(bridge, "keyvault.setup") == "failed")
    }

    /// New Space's options, while starting and once ready: never an
    /// answer the page's wizard can't run on.
    @Test func createOptionsHaveTheirShapeWhileStartingAndReady() async throws {
        let doc = try Self.shapes()
        let schema = try #require((doc["webkit"] as? [String: Any])?["spaces.createOptions"])
        let defs = doc["$defs"] as? [String: Any] ?? [:]
        let (bridge, model, _) = try await fixtureBridge()
        // While the launch is still at the Keychain: at once, with what is known.
        let starting = try Self.json(WebUIBridge.createOptions(env: model.knownNewSpaceEnv(), macosVmsRunning: nil, pending: true))
        #expect(JSONShape.validate(schema, starting, defs: defs) == [])
        let routed = try Self.json(try await bridge.handle("spaces.createOptions", [:]))
        #expect(JSONShape.validate(schema, routed, defs: defs) == [])
        let ready = try Self.json(WebUIBridge.createOptions(env: await model.newSpaceEnv(), macosVmsRunning: 1, pending: false))
        #expect(JSONShape.validate(schema, ready, defs: defs) == [])
        // Such an answer fails.
        let broken: [String: Any] = ["local": NSNull(), "gpus": NSNull(), "cloudPricing": NSNull(), "experiments": [:], "maxCpus": 8]
        #expect(JSONShape.validate(schema, broken, defs: defs).sorted() == ["$.env: missing", "$.local: expected object, got null"])
    }

    @Test func startupStatesHaveTheirShape() throws {
        let doc = try Self.shapes()
        let schema = try #require((doc["webkit"] as? [String: Any])?["startup.get"])
        let defs = doc["$defs"] as? [String: Any] ?? [:]
        for phase in [StartupModel.Phase.starting, .needsKeychain(locked: false), .ready] {
            let state = try Self.json(WebUIBridge.startupState(StartupModel(phase: phase)))
            #expect(JSONShape.validate(schema, state, defs: defs) == [], "\(phase)")
        }
    }
}

/// The JSON Schema subset `contracts/schema.ts` writes and validates:
/// `type`, `properties`, `required`, `additionalProperties`, `items`,
/// `enum`, `anyOf` and `$ref` (`#/$defs/<name>`). Errors read like the web
/// side's (`$.local: expected object, got null`).
enum JSONShape {
    static func isBool(_ v: Any) -> Bool {
        guard let n = v as? NSNumber else { return false }
        return CFGetTypeID(n) == CFBooleanGetTypeID()
    }

    static func typeName(_ v: Any) -> String {
        if v is NSNull { return "null" }
        if isBool(v) { return "boolean" }
        if v is NSNumber { return "number" }
        if v is String { return "string" }
        if v is [Any] { return "array" }
        if v is [String: Any] { return "object" }
        return "\(type(of: v))"
    }

    static func matches(_ t: String, _ v: Any) -> Bool {
        switch t {
        case "integer": return !isBool(v) && (v as? NSNumber).map { $0.doubleValue.rounded() == $0.doubleValue } ?? false
        case "number": return !isBool(v) && v is NSNumber
        default: return typeName(v) == t
        }
    }

    static func same(_ a: Any, _ b: Any) -> Bool {
        if a is NSNull || b is NSNull { return a is NSNull && b is NSNull }
        if isBool(a) != isBool(b) { return false }
        if let x = a as? String, let y = b as? String { return x == y }
        if let x = a as? NSNumber, let y = b as? NSNumber { return x == y }
        return false
    }

    static func show(_ v: Any) -> String {
        if let s = v as? String { return "\"\(s)\"" }
        if v is NSNull { return "null" }
        if isBool(v) { return (v as? Bool) == true ? "true" : "false" }
        return "\(v)"
    }

    static func validate(_ schemaAny: Any, _ value: Any, defs: [String: Any], path: String = "$") -> [String] {
        guard let schema = schemaAny as? [String: Any] else { return ["\(path): bad shape"] }
        if let ref = schema["$ref"] as? String {
            let name = ref.replacingOccurrences(of: "#/$defs/", with: "")
            guard let target = defs[name] else { return ["\(path): unknown shape \(ref)"] }
            return validate(target, value, defs: defs, path: path)
        }
        if let any = schema["anyOf"] as? [Any] {
            let each = any.map { validate($0, value, defs: defs, path: path) }
            if each.contains(where: \.isEmpty) { return [] }
            return each.min { $0.count < $1.count } ?? ["\(path): matches no alternative"]
        }
        if let options = schema["enum"] as? [Any], !options.contains(where: { same($0, value) }) {
            return ["\(path): \(show(value)) is not one of \(options.map(show).joined(separator: ", "))"]
        }
        if let t = schema["type"] {
            let types = (t as? [String]) ?? [(t as? String) ?? ""]
            if !types.contains(where: { matches($0, value) }) {
                return ["\(path): expected \(types.joined(separator: " or ")), got \(typeName(value))"]
            }
        }
        var errors: [String] = []
        if let array = value as? [Any], let items = schema["items"] {
            for (i, x) in array.enumerated() { errors += validate(items, x, defs: defs, path: "\(path)[\(i)]") }
        } else if let object = value as? [String: Any] {
            let properties = schema["properties"] as? [String: Any] ?? [:]
            for k in schema["required"] as? [String] ?? [] where object[k] == nil { errors.append("\(path).\(k): missing") }
            for (k, s) in properties.sorted(by: { $0.key < $1.key }) {
                if let x = object[k] { errors += validate(s, x, defs: defs, path: "\(path).\(k)") }
            }
            if let extra = schema["additionalProperties"] as? [String: Any] {
                for (k, x) in object.sorted(by: { $0.key < $1.key }) where properties[k] == nil {
                    errors += validate(extra, x, defs: defs, path: "\(path).\(k)")
                }
            }
        }
        return errors
    }
}
