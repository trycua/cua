// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
import CuaSpacesStreaming
import Foundation
import Observation
import WebKit

/// What the bridge asks the SwiftUI scenes to do (they own `openWindow`).
@MainActor
public struct WebUIActions {
    /// A Space's desktop in its own native window (video stays native).
    public var openSpace: (String) -> Void
    /// The native main window (a passphrase is chosen there).
    public var openMain: () -> Void

    public init(openSpace: @escaping (String) -> Void, openMain: @escaping () -> Void) {
        self.openSpace = openSpace
        self.openMain = openMain
    }
}

/// The web UI's native bridge: the `cua` script message handler.
///
/// Request (web to native), through
/// `window.webkit.messageHandlers.cua.postMessage(request)`, which resolves
/// with the response:
///
///     { id: string, method: string, args?: object }
///     -> { id, ok: true, result } | { id, ok: false, error: { code, message } }
///
/// `code` is `unimplemented` for a method this host does not route (the UI
/// falls back to demo data), `bad_args`, `not_found`, `unsupported` (the
/// app can't answer it here, such as agents without the daemon) or `failed`.
///
/// Events (native to web): `window.dispatchEvent(new CustomEvent("cua:event",
/// { detail: { event, payload } }))`, where `event` is `spaces.changed`,
/// `machines.changed`, `keyvault.changed`, `session.changed`,
/// `settings.changed` or `agents.changed`; the UI asks again for what it
/// shows. `spaces.createProgress` carries a create's progress (the page's
/// pending id), and `spaces.newRequested` (`{on}`) asks the page to open
/// its New Space wizard (the app's New Space menu items, the notch).
/// `settings.openRequested` asks the page to open its Settings (the app
/// menu's Settings command, ⌘,, while the New UI window is in front).
///
/// Every method routes to the view models the SwiftUI windows use, so a
/// decision is the app core's either way. `spaces.cancelCreate`,
/// `host.status`, `keyvault.approve`, `keyvault.deny` and
/// `keyvault.revokeGrant` take the same names and arguments as the Electron
/// router's channels (`cua:<name>`, `HostOperations` in the web bridge's
/// `protocol.ts`). The Keyvault's widening paths are
/// the native ones: the unlock prompt is a native alert, and the daemon asks
/// for Touch ID. No secret value crosses the bridge (the app has none).
@MainActor
final class WebUIBridge: NSObject, WKScriptMessageHandlerWithReply {
    static let name = "cua"

    /// The methods this host routes, in the order `app.info` lists them.
    static let methods = [
        "app.info",
        "session.get", "session.signIn", "session.signOut",
        "spaces.list", "spaces.open",
        "spaces.createOptions", "spaces.create",
        "spaces.setPower", "spaces.delete", "spaces.cancelCreate",
        "machines.list", "host.status",
        "agents.list", "agents.runs", "agents.events", "agents.pause", "agents.resume",
        "agents.setup", "agents.configure",
        "agentKeys.list", "agentKeys.set", "agentKeys.remove",
        // New Space's address form and Connect a cloud, Teleport and Share,
        // Cua Volume, Settings and Notifications, a Space's detail and This
        // machine (WebUIBridge+Pages.swift).
        "spaces.add", "clouds.status", "clouds.test", "clouds.connect",
        "teleport.catalog", "teleport.entryForPath", "teleport.windows", "teleport.remoteWindows",
        "teleport.icon", "teleport.thumbnail", "teleport.plan", "teleport.run", "teleport.sites",
        "teleport.remembered", "teleport.streamWindow", "sharing.list", "sharing.share", "sharing.unshare",
        "volume.overview", "volume.storage", "volume.storageSet", "volume.mount", "volume.unmount",
        "volume.approve", "volume.deny", "volume.revoke", "volume.resolve", "volume.reveal",
        "agents.setupDriver",
        "about.get", "about.set", "about.checkNow", "loginItem.get", "loginItem.set", "loginItem.openSettings",
        "devices.get", "devices.enroll", "devices.checkEnrolled", "devices.approve", "devices.rename",
        "devices.revoke", "devices.confirmMachine", "storage.get", "storage.run",
        "notifications.list", "notifications.markAllRead",
        "telemetry.track", "spaces.usage", "spaces.windows", "stream.pip",
        // A Space's thumbnail (WebUIBridge+Thumbnails.swift).
        "spaces.thumbnail",
        // A Space's drop well: Send file… and dropped files (WebUIBridge+Files.swift).
        "spaces.chooseFiles", "spaces.droppedFiles", "spaces.sendFiles",
        "host.setUp", "host.action", "host.openSettings",
        "settings.get", "settings.choose",
        "keyvault.get", "keyvault.lock", "keyvault.unlock",
        "keyvault.unlockVault", "keyvault.setup", "keyvault.setDisabled",
        "keyvault.approve", "keyvault.deny", "keyvault.revokeGrant",
        "keyvault.showItems", "keyvault.delete", "keyvault.run", "keyvault.dismiss",
        "window.setBackgroundColor", "window.setDragRegions",
        // The launch (WebUIBridge+Startup.swift).
        "startup.get", "startup.act",
    ]

    let model: AppModel
    var actions: WebUIActions?
    weak var host: WebUIWindowController?
    /// What each `followChanges` event last saw.
    private var signatures: [String: String] = [:]
    /// Teleport: the SDK's entries and plans the page names by id and by
    /// plan, as `TeleportModel` keeps them between its steps. Bounded as
    /// the model's are: the entries are the latest catalog read (plus the
    /// apps picked from a window since), and a Space keeps only its latest
    /// plan, which a finished or failed run drops.
    var teleportEntries: [String: TeleportCatalogEntry] = [:]
    var teleportPlans: [String: (json: String, plan: TeleportPlan, providerId: String?)] = [:]
    /// Asks whether to delete (the native alert); tests answer it.
    var askDelete: (@MainActor (KvDeleteConfirm) async -> Bool)?
    /// Each Space's stream rows and picture-in-picture panels, as its
    /// detail view keeps them. A deleted Space's are closed and dropped
    /// (`pruneStreams`).
    var streams: [String: (rows: StreamRowsModel, pips: StreamPiPSet)] = [:]
    /// The page has called in since it last loaded (its listener is up).
    var pageListening = false
    /// A New Space asked for before the page listened (`on`: "Run on").
    var pendingNewSpace: String??
    /// Settings asked for before the page listened.
    var pendingSettings = false
    /// Local paths the user picked or dropped here: the only ones
    /// `spaces.sendFiles` sends (WebUIBridge+Files.swift).
    var offeredPaths: Set<String> = []
    /// "Send file…"'s picker (a test sets its own).
    var pickFiles: @MainActor () -> [URL]? = WebUIBridge.openPanel
    /// The pasteboard of the last drag onto the window (a test sets its own).
    var dragPasteboard: @MainActor () -> NSPasteboard = { NSPasteboard(name: .drag) }
    /// Origins allowed to call (the bundled UI, and the dev server in a
    /// debug build).
    let allowedOrigins: Set<String>

    init(model: AppModel, allowedOrigins: Set<String>) {
        self.model = model
        self.allowedOrigins = allowedOrigins
    }

    struct Failure: Error {
        let code: String
        let message: String
        /// A failed host setup or This machine button in plain words: the
        /// error then also carries its `title`, `details` (the raw error)
        /// and `actionLabel` (Retry, or Sign In).
        var presented: HostSetupFailure?
        static func unimplemented(_ method: String) -> Failure {
            Failure(code: "unimplemented", message: "\(method) is not available in this host")
        }
        static func badArgs(_ message: String) -> Failure { Failure(code: "bad_args", message: message) }
        static func notFound(_ message: String) -> Failure { Failure(code: "not_found", message: message) }
        static func unsupported(_ message: String) -> Failure { Failure(code: "unsupported", message: message) }
    }

    func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage,
                               replyHandler: @escaping @MainActor @Sendable (Any?, String?) -> Void) {
        let request = message.body as? [String: Any] ?? [:]
        let id = request["id"] as? String ?? (request["id"].map { "\($0)" } ?? "")
        let origin = message.frameInfo.securityOrigin
        let originKey = "\(origin.protocol)://\(origin.host)\(origin.port == 0 ? "" : ":\(origin.port)")"
        guard message.frameInfo.isMainFrame, allowedOrigins.contains(originKey) else {
            replyHandler(Self.reply(id: id, error: Failure(code: "forbidden", message: "origin not allowed")), nil)
            return
        }
        guard let method = request["method"] as? String else {
            replyHandler(Self.reply(id: id, error: .badArgs("missing method")), nil)
            return
        }
        let args = request["args"] as? [String: Any] ?? [:]
        pageIsListening()
        Task { @MainActor in
            do {
                let result = try await self.handle(method, args)
                replyHandler(["id": id, "ok": true, "result": result], nil)
            } catch let f as Failure {
                replyHandler(Self.reply(id: id, error: f), nil)
            } catch {
                replyHandler(Self.reply(id: id, error: Failure(code: "failed",
                                                               message: LiveSpacesBackend.words(error))), nil)
            }
        }
    }

    /// The page is listening now: a New Space or Settings it was asked for
    /// before it loaded goes out.
    func pageIsListening() {
        pageListening = true
        if let on = pendingNewSpace {
            pendingNewSpace = nil
            DispatchQueue.main.async { [weak self] in self?.emitNewSpace(on: on) }
        }
        if pendingSettings {
            pendingSettings = false
            DispatchQueue.main.async { [weak self] in self?.emitSettings() }
        }
    }

    static func reply(id: String, error: Failure) -> [String: Any] {
        var e: [String: Any] = ["code": error.code, "message": error.message]
        if let p = error.presented {
            e["title"] = p.title
            e["details"] = p.details
            e["actionLabel"] = p.actionLabel
        }
        return ["id": id, "ok": false, "error": e]
    }

    // MARK: - Routing

    func handle(_ method: String, _ args: [String: Any]) async throws -> Any {
        switch method {
        case "app.info": return appInfo()

        case "session.get": return session()
        case "session.signIn":
            // The browser flow can take minutes: answer now, and
            // `session.changed` follows when it ends.
            Task { @MainActor in await model.beginSignIn() }
            return session()
        case "session.signOut":
            await model.signOut()
            return session()

        case "spaces.list": return spaces()
        case "spaces.open":
            let id = try string(args, "id")
            guard model.spaces.contains(where: { $0.id == id }) else { throw Failure.notFound("no Space \(id)") }
            guard let actions else { throw Failure.unimplemented(method) }
            actions.openSpace(id)
            return NSNull()
        case "spaces.createOptions": return await createOptions()
        case "spaces.create": return try await create(args)
        case "spaces.setPower":
            let space = try space(args)
            guard let on = args["on"] as? Bool else { throw Failure.badArgs("on: boolean") }
            model.setPower(space, on: on)
            return spaces()
        case "spaces.delete":
            let space = try space(args)
            model.delete(space, removeOnly: args["removeOnly"] as? Bool ?? false)
            return spaces()
        case "spaces.cancelCreate":
            return cancelCreate(try string(args, "pendingId"))

        case "machines.list":
            if model.host.state == nil, model.servicesIn { await model.host.refresh() }
            // Who the relay sees connected now (a host that rejoined after
            // sign-in is online), not at the last Devices page visit; not
            // before the live services are in (answer from memory then).
            if model.servicesIn { await model.devices.refreshIfStale() }
            return machines()
        case "host.status":
            if model.host.state == nil, model.servicesIn { await model.host.refresh() }
            return hostStatus()

        case "agents.list", "agents.runs", "agents.events", "agents.pause", "agents.resume",
             "agents.setup", "agents.configure":
            return try await agents(method, args)

        case "settings.get": return settings()
        case "settings.choose":
            let row = try string(args, "row"), option = try string(args, "option")
            if row == "update-channel" {
                // Settings → About's channel (saved in `AppSettings`).
                guard model.updates.updater != nil else { throw Failure.unsupported("Updates are off in this build") }
                model.updates.choose(channel: option)
            } else if row.hasPrefix("experiment:") {
                model.chooseExperiment(row: row, option: option)
            } else if row == "welcome" || row.hasPrefix("agent:") {
                // General's "Show again": the welcome window, as the native
                // Settings button opens it. An AI agents row's Configure or
                // Remove: what its native button does.
                await model.press(row: row)
            } else {
                await model.choose(row: row, option: option)
            }
            return settings()

        case "keyvault.get":
            // The broker, read again (the page asks when it shows the
            // Keyvault and after `keyvault.changed`); not while launching.
            if model.servicesIn { await model.keyvault.refresh() }
            return keyvault()
        case "keyvault.lock":
            await model.keyvault.lock(ids: try strings(args, "ids"))
            return try keyvaultResult()
        case "keyvault.unlock":
            try await unlockItems(try strings(args, "ids"), name: args["name"] as? String)
            return try keyvaultResult()
        case "keyvault.unlockVault":
            try await unlockVault()
            return try keyvaultResult()
        case "keyvault.setup":
            let key = try await setUpVault()
            return ["recoveryKey": key.map { $0 as Any } ?? NSNull()]
        case "keyvault.setDisabled":
            guard let disabled = args["disabled"] as? Bool else { throw Failure.badArgs("disabled: boolean") }
            await model.keyvault.setDisabled(disabled)
            return try keyvaultResult()
        case "keyvault.approve":
            // The daemon asks for Touch ID or the login password; the page
            // sends the item ids the user ticked (null: all, as asked).
            let request = try pendingRequest(args)
            let items: [String]? = args["items"] == nil || args["items"] is NSNull ? nil : try strings(args, "items")
            let outcome = await model.keyvault.run(.approve(requestId: request, items: items))
            guard case .granted(let grant) = outcome else { throw keyvaultFailure("Approve did not complete") }
            return BridgeValue.encode(grant)
        case "keyvault.deny":
            await model.keyvault.deny(try pendingRequest(args))
            if let error = model.keyvault.error { throw Failure(code: "failed", message: error) }
            return NSNull()
        case "keyvault.revokeGrant":
            let outcome = await model.keyvault.run(.revokeGrant(id: try string(args, "id")))
            guard case .revoked(let count) = outcome else { throw keyvaultFailure("Revoke did not complete") }
            return NSNumber(value: count)
        case "keyvault.showItems":
            // The names, behind the daemon's Touch ID (the list's "Show Items").
            await model.keyvault.showItems()
            return try keyvaultResult()
        case "keyvault.delete":
            try await deleteItems(try strings(args, "ids"))
            return try keyvaultResult()
        case "keyvault.run":
            // An Access row's own command; nothing else runs from the page.
            await model.keyvault.run(try accessCommand(try object(args, "command")))
            return try keyvaultResult()
        case "keyvault.dismiss":
            model.keyvault.dismiss(try strings(args, "imports"))
            return keyvault()

        case "window.setBackgroundColor":
            guard let color = NSColor(webHex: try string(args, "color")) else {
                throw Failure.badArgs("color: #rrggbb")
            }
            host?.setBackground(color)
            return NSNull()
        case "window.setDragRegions":
            guard let rects = args["rects"] as? [[String: Any]] else { throw Failure.badArgs("rects: [{x,y,width,height}]") }
            host?.setDragRegions(rects.compactMap(Self.rect))
            return NSNull()

        default:
            if let result = try routeStartup(method, args) { return result }
            if let result = try await routeThumbnail(method, args) { return result }
            if let result = try await routeFiles(method, args) { return result }
            if let result = try await routePage(method, args) { return result }
            throw Failure.unimplemented(method)
        }
    }

    // MARK: - Results

    func appInfo() -> [String: Any] {
        let info = Bundle.main.infoDictionary ?? [:]
        return [
            "platform": "macos",
            "host": "swiftui",
            "version": info["CuaVersion"] as? String ?? info["CFBundleShortVersionString"] as? String ?? "dev",
            "experiments": BridgeValue.encode(model.settings.experiments),
            "methods": Self.methods,
        ]
    }

    func session() -> [String: Any] {
        [
            "identity": model.identity ?? NSNull(),
            "signedIn": model.identity != nil,
            "cloudConfigured": model.cloudConfigured,
            "signIn": BridgeValue.encode(model.signIn),
            "chrome": BridgeValue.encode(model.chrome),
        ]
    }

    func spaces() -> [String: Any] {
        [
            "loaded": model.loaded,
            "selectedId": model.selectedSpaceId ?? NSNull(),
            "sidebar": BridgeValue.encode(model.sidebar),
            "spaces": model.spaces.map { space in
                [
                    "space": BridgeValue.encode(space),
                    "detail": BridgeValue.encode(model.detail(space)),
                    "deleting": model.isDeleting(space.id),
                ] as [String: Any]
            },
            "statusLine": model.statusLine,
            // Why the list may be out of date (null when the last read worked).
            "rosterError": model.rosterError ?? NSNull(),
        ]
    }

    func machines() -> [String: Any] {
        let sidebar = model.sidebar
        return [
            "thisMachine": BridgeValue.encode(sidebar.thisMachine),
            "sections": BridgeValue.encode(sidebar.sections),
            "devices": model.devices.snapshot == nil ? NSNull() : BridgeValue.encode(model.devices.view),
            "accessNotice": BridgeValue.encode(model.devices.accessNotice),
            "signedIn": model.devices.signedIn,
            // The Machines page's "This machine" panel (`host.panel`).
            "host": hostStatus(),
            // What each machine on the relay said its hostname is, so the
            // page lists a machine and its enrolled device once.
            "hostnames": relayHostnames(),
            // Who the relay sees connected, by machine id: the app's own
            // connect probe can time out on a host the relay reaches.
            "presence": model.devices.relayOnline,
            "deviceStates": model.devices.deviceStates,
        ]
    }

    /// `relay:<machine>` rows' reported hostnames, by machine id.
    func relayHostnames() -> [String: String] {
        var out: [String: String] = [:]
        for space in model.spaces where space.id.hasPrefix("relay:") && space.host == nil {
            if let name = model.backend.reportedHostname(id: space.id) { out[String(space.id.dropFirst(6))] = name }
        }
        return out
    }

    func settings() -> [String: Any] {
        [
            "page": BridgeValue.encode(model.settingsPageWithStorage),
            "experiments": BridgeValue.encode(model.experimentsPage),
            // Settings → About's channel; null while this build has no updater.
            "updateChannel": model.updates.updater == nil ? NSNull() : (model.updates.channel == .beta ? "beta" : "stable"),
        ]
    }

    // MARK: - New Space (the page's wizard; the app core's create)

    /// Asks the page to open its New Space wizard ("Run on" set to `on`
    /// when given); queued until the page listens.
    func requestNewSpace(on: String?) {
        if pageListening { emitNewSpace(on: on) } else { pendingNewSpace = .some(on) }
    }

    func emitNewSpace(on: String?) {
        host?.emit("spaces.newRequested", payload: ["on": on.map { $0 as Any } ?? NSNull()])
    }

    // MARK: - Settings (the app menu's command while this window is in front)

    /// Asks the page to go to its Settings; queued until the page listens.
    func requestSettings() {
        if pageListening { emitSettings() } else { pendingSettings = true }
    }

    func emitSettings() {
        host?.emit("settings.openRequested")
    }

    /// `spaces.createOptions`: the env the native New Space sheet opens
    /// with (`AppModel.newSpaceEnv`: this Mac's runtimes, storage and GPUs,
    /// your machines that provide Spaces, your clouds, pricing), which the
    /// page's wizard runs on as it is, in the shape of the web bridge's
    /// `NewSpaceOptions` (`local`, `gpus` and `cloudPricing` from the same
    /// env, for readers that don't take `env`), and the macOS VMs running
    /// on this Mac (`macosVmsRunning`: Apple's limit of two counts every
    /// one, Spaces or not).
    ///
    /// Until the live services are in (the launch is still at the Keychain
    /// or starting the daemon), every probe would wait for them, longer
    /// than the page waits for an answer: it answers at once with what is
    /// known and `pending: true`, and the page asks again once the launch
    /// is ready (`startup.changed`).
    func createOptions() async -> [String: Any] {
        guard model.servicesIn else {
            return Self.createOptions(env: model.knownNewSpaceEnv(), macosVmsRunning: nil, pending: true)
        }
        let env = await model.newSpaceEnv()
        return Self.createOptions(env: env, macosVmsRunning: model.runningMacosVms, pending: false)
    }

    static func createOptions(env: AppWizardEnv, macosVmsRunning: Int?, pending: Bool) -> [String: Any] {
        var out: [String: Any] = [
            "local": [
                "available": env.localAvailable,
                "backends": env.localBackends ?? [],
                "error": env.localReason.map { $0 as Any } ?? NSNull(),
                "hostArch": env.hostArch.map { $0 as Any } ?? NSNull(),
                "storage": BridgeValue.encode(env.storage),
            ] as [String: Any],
            "gpus": BridgeValue.encode(env.gpus),
            "cloudPricing": BridgeValue.encode(env.cloudPricing),
            "experiments": BridgeValue.encode(env.experiments),
            "maxCpus": NSNumber(value: env.maxCpus),
            "env": BridgeValue.encode(env),
            "macosVmsRunning": macosVmsRunning.map { NSNumber(value: $0) } ?? NSNull(),
        ]
        if pending { out["pending"] = true }
        return out
    }

    /// `spaces.create {config, pendingId, os?}`: the core's create
    /// arguments (`wizard.createArgs`), run as the native sheet's Create
    /// runs them (`AppModel.runCreate`: the pending row in the list and the
    /// notch, the SDK's progress, Cancel through `spaces.cancelCreate`).
    /// Progress goes to the page as `spaces.createProgress`; the answer is
    /// the new Space, or `cancelled`.
    func create(_ args: [String: Any]) async throws -> Any {
        let pendingId = try string(args, "pendingId")
        guard appCreatesIsPending(id: pendingId) else { throw Failure.badArgs("pendingId: pending:<id>") }
        guard let config = args["config"] as? [String: Any] else { throw Failure.badArgs("config: object") }
        let createArgs = try Self.createArgs(config, defaultOn: model.settings.defaultLocation == .cloud ? "cloud" : "local")
        // The page sends the plan's OS (the pending row's icon).
        let os = (args["os"] as? String).flatMap(AppSpaceOs.init(word:)) ?? .unknown
        let id: String
        do {
            id = try await model.runCreate(createArgs, os: os, pendingId: pendingId) { [weak self] p in
                self?.host?.emit("spaces.createProgress", payload: Self.progress(p, pendingId: pendingId))
            }
        } catch where isCancelled(error) {
            throw Failure(code: "cancelled", message: "cancelled")
        }
        if let space = model.spaces.first(where: { $0.id == id }) { return BridgeValue.encode(space) }
        return ["id": id, "name": createArgs.name ?? id, "os": BridgeValue.encode(os), "status": "running",
                "detail": "", "lastUsedAt": Double(AppModel.nowMs())] as [String: Any]
    }

    /// The SDK's create progress as the bridge's `CreateProgress`.
    static func progress(_ p: SpaceCreateProgress, pendingId: String) -> [String: Any] {
        [
            "pendingId": pendingId, "phase": p.phase, "detail": p.detail,
            "fraction": p.fraction.map { $0 as Any } ?? NSNull(),
            "bytesDone": p.bytesDone.map { NSNumber(value: $0) } ?? NSNull(),
            "bytesTotal": p.bytesTotal.map { NSNumber(value: $0) } ?? NSNull(),
            "bytesPerSecond": p.bytesPerSecond.map { $0 as Any } ?? NSNull(),
        ]
    }

    /// The page's `SpaceCreateConfig` (the core's `CreateSpaceArgs`) as
    /// the FFI record.
    static func createArgs(_ c: [String: Any], defaultOn: String) throws -> AppCreateSpaceArgs {
        guard let image = c["image"] as? String, !image.isEmpty else { throw Failure.badArgs("config.image: string") }
        guard let kind = AppSpaceKind(word: c["kind"] as? String ?? "") else { throw Failure.badArgs("config.kind: container | vm") }
        let runtime: AppRuntime
        switch c["runtime"] as? String ?? "auto" {
        case "auto": runtime = .auto
        case "gvisor": runtime = .gvisor
        case "runc": runtime = .runc
        case "qemu": runtime = .qemu
        case "lume": runtime = .lume
        case "kubevirt": runtime = .kubevirt
        default: throw Failure.badArgs("config.runtime: auto | gvisor | runc | qemu | lume | kubevirt")
        }
        func count(_ key: String) -> UInt32? { (c[key] as? NSNumber).map { $0.uint32Value } }
        let text = { (key: String) in (c[key] as? String).flatMap { $0.isEmpty ? nil : $0 } }
        return AppCreateSpaceArgs(
            image: image, on: text("on") ?? defaultOn, kind: kind, runtime: runtime, name: text("name"),
            cpus: count("cpus"), memoryMb: count("memoryMb"), diskGb: count("diskGb"),
            spacesd: c["spacesd"] as? Bool ?? true, gpu: text("gpu"))
    }

    // MARK: - Agents (the Agents page and Settings → AI agents)

    /// The `agents.*` methods: the persistent agents (`PersistentModel`), a
    /// Space's runs (`AgentRunsModel`), a run's events (the daemon's
    /// `agent_events` tool) and the coding agents on this Mac (the Settings
    /// "AI agents" rows).
    func agents(_ method: String, _ args: [String: Any]) async throws -> Any {
        let persistent = model.persistent
        switch method {
        case "agents.list":
            guard persistent.canCallTools else { throw Failure.unsupported("Agents need the cua daemon") }
            return try await persistent.listAgents()
        case "agents.runs":
            let runs = AgentRunsModel(backend: model.backend, spaceId: try string(args, "spaceId"))
            await runs.refresh()
            switch runs.load {
            case .ready(let rows): return BridgeValue.encode(rows)
            case .failed(let message): throw Failure(code: "failed", message: message)
            case .loading: return [Any]()
            }
        case "agents.events":
            guard persistent.canCallTools else { throw Failure.unsupported("A run's conversation needs the cua daemon") }
            let cursor = (args["cursor"] as? NSNumber)?.uint64Value ?? 0
            let max = (args["max"] as? NSNumber)?.uint32Value
            return try await persistent.runEvents(space: try string(args, "spaceId"), runId: try string(args, "runId"),
                                                  cursor: cursor, max: max)
        case "agents.pause", "agents.resume":
            let name = try string(args, "name")
            guard !persistent.agentsState.busy else { throw Failure(code: "failed", message: "Another change is still running") }
            await persistent.send(method == "agents.pause" ? .pause(name: name) : .resume(name: name))
            if let error = persistent.agentsState.error { throw Failure(code: "failed", message: error) }
            return NSNull()
        case "agents.setup":
            guard model.agentSetup != nil else { throw Failure.unsupported("Agent setup is not available in this build") }
            await model.reloadAgents()
            return BridgeValue.encode(model.agentRows ?? [])
        case "agents.configure":
            guard model.agentSetup != nil else { throw Failure.unsupported("Agent setup is not available in this build") }
            if args["agents"] == nil || args["agents"] is NSNull {
                await model.configureAllAgents()
            } else {
                await model.agentAction(try strings(args, "agents"), remove: false)
            }
            return BridgeValue.encode(model.agentRows ?? [])
        default:
            throw Failure.unimplemented(method)
        }
    }

    func keyvault() -> [String: Any] {
        let kv = model.keyvault
        return [
            "availability": kv.overview.availability,
            "page": BridgeValue.encode(kv.page),
            "sidebar": BridgeValue.encode(kv.sidebar),
            "list": BridgeValue.encode(kv.list),
            "overview": Self.overview(kv.overview),
            "dismissed": kv.dismissed,
            "busy": kv.busy,
            "error": kv.error ?? NSNull(),
        ]
    }

    /// The broker's redacted overview, for the web UI's own vault list (the
    /// core's `list` view does not carry the items). Items lose `blob`, so
    /// nothing but names and policy crosses the bridge.
    static func overview(_ overview: KeyvaultOverview) -> Any {
        guard var out = BridgeValue.encode(overview) as? [String: Any] else { return NSNull() }
        if let items = out["items"] as? [[String: Any]] {
            out["items"] = items.map { item in
                var item = item
                item.removeValue(forKey: "blob")
                return item
            }
        }
        return out
    }

    /// A Space still being created: the core's create state starts the
    /// cancel (the row shows Cancelling, then goes). The answer has the
    /// SDK's cancel words: `cancelled`, `not_creating` or `already_created`.
    func cancelCreate(_ id: String) -> [String: Any] {
        let pending = model.creates.pending.first { $0.id == id }
        let state: String
        switch pending {
        case .some(let p) where p.error == nil && p.spaceId == nil:
            if !p.cancelling { model.cancelCreate(id) }
            state = "cancelled"
        case .some(let p) where p.spaceId != nil: state = "already_created"
        default: state = model.spaces.contains { $0.id == id } && !id.hasPrefix("pending:") ? "already_created" : "not_creating"
        }
        return ["id": id, "state": state, "message": ""]
    }

    /// This machine as a host (`HostModel.state`, the core's flattened
    /// `HostStatus`, with the signed-in `account` it was checked against),
    /// plus its relay machine id and what a running setup or Sign In waits
    /// for (`progress`); null until it answered.
    func hostStatus() -> Any {
        guard let state = model.host.state, var out = BridgeValue.encode(state) as? [String: Any] else { return NSNull() }
        out["machineId"] = model.host.machineId ?? NSNull()
        out["progress"] = model.host.progress ?? NSNull()
        return out
    }

    /// `requestId`, which must be waiting in the broker's overview.
    func pendingRequest(_ args: [String: Any]) throws -> String {
        let id = try string(args, "requestId")
        guard model.keyvault.overview.pending.contains(where: { $0.id == id }) else {
            throw Failure.notFound("no waiting request \(id)")
        }
        return id
    }

    func keyvaultFailure(_ fallback: String) -> Failure {
        Failure(code: "failed", message: model.keyvault.error ?? fallback)
    }

    /// The Keyvault after an action, or the action's error.
    func keyvaultResult() throws -> [String: Any] {
        if let error = model.keyvault.error { throw Failure(code: "failed", message: error) }
        return keyvault()
    }

    // MARK: - Keyvault (native prompts, the daemon's Touch ID)

    /// The core's unlock prompt (unless the user chose Never ask again) as a
    /// native alert, then the same unlock command the SwiftUI list sends;
    /// the daemon asks for Touch ID once for the batch.
    func unlockItems(_ ids: [String], name: String?) async throws {
        guard !ids.isEmpty else { throw Failure.badArgs("ids: non-empty") }
        let kv = model.keyvault
        guard let prompt = kvUnlockPrompt(overview: kv.overview, count: UInt32(ids.count), name: name) else {
            await kv.run(kvUnlockCommand(ids: ids))
            return
        }
        switch await host?.ask(prompt) ?? .deny {
        case .deny: throw Failure(code: "cancelled", message: "Unlock cancelled")
        case .allow: await kv.run(kvUnlockCommand(ids: ids))
        case .neverAskAgain:
            await kv.run(.setSkipUnlockPrompt(on: true))
            await kv.run(kvUnlockCommand(ids: ids))
        }
    }

    /// Delete: the core's confirmation (it says when copies in Spaces are
    /// wiped too) as a native alert, then the same delete the SwiftUI list
    /// sends. Declined: `cancelled`, nothing changes.
    func deleteItems(_ ids: [String]) async throws {
        guard !ids.isEmpty else { throw Failure.badArgs("ids: non-empty") }
        let kv = model.keyvault
        kv.requestDelete(ids: ids)
        guard let pending = kv.deleteConfirm else { throw Failure.badArgs("ids: none of these items") }
        // The page's alert replaces the sheet the Keyvault list would show.
        kv.deleteConfirm = nil
        let confirmed: Bool
        if let askDelete { confirmed = await askDelete(pending.confirm) }
        else { confirmed = await host?.ask(pending.confirm) ?? false }
        guard confirmed else { throw Failure(code: "cancelled", message: "Delete cancelled") }
        kv.deleteConfirm = pending
        await kv.confirmDelete()
    }

    /// The commands an Access row sends: revoke a grant, remove a rule, wipe
    /// a Space's copies.
    func accessCommand(_ c: [String: Any]) throws -> KvCommand {
        switch c["type"] as? String {
        case "revoke-grant": return .revokeGrant(id: try string(c, "id"))
        case "remove-rule": return .removeRule(id: try string(c, "id"))
        case "release": return .release(target: try string(c, "target"))
        default: throw Failure.badArgs("command: revoke-grant, remove-rule or release")
        }
    }

    /// Unlocks (or sets up) the vault with Touch ID, the daemon's prompt. A
    /// passphrase vault unlocks in the native window: a passphrase never
    /// crosses the bridge.
    func unlockVault() async throws {
        let kv = model.keyvault
        guard let form = kv.page.form else { return }
        guard form.method == .touchId else {
            actions?.openMain()
            model.selection = .keyvault(.category(category: .all))
            throw Failure(code: "native_only", message: "Enter the passphrase in the Cua Spaces window")
        }
        await kv.submitCredential()
    }

    /// "Set up Keyvault" on the web page: the core's setup form with Touch
    /// ID, as the SwiftUI form sends it. A passphrase is typed only in the
    /// native form (it never crosses the bridge). Answers the recovery key
    /// to show once.
    func setUpVault() async throws -> String? {
        let kv = model.keyvault
        guard let form = kv.page.form, form.mode == .setup else {
            throw Failure(code: "failed", message: "Keyvault is already set up")
        }
        guard form.method == .touchId else {
            actions?.openMain()
            model.selection = .keyvault(.category(category: .all))
            throw Failure(code: "native_only", message: "Choose a passphrase in the Cua Spaces window")
        }
        kv.recoveryKey = nil
        await kv.submitCredential()
        if let error = kv.error { throw Failure(code: "failed", message: error) }
        return kv.recoveryKey
    }

    // MARK: - Arguments

    func string(_ args: [String: Any], _ key: String) throws -> String {
        guard let v = args[key] as? String, !v.isEmpty else { throw Failure.badArgs("\(key): string") }
        return v
    }

    func strings(_ args: [String: Any], _ key: String) throws -> [String] {
        guard let v = args[key] as? [String] else { throw Failure.badArgs("\(key): string[]") }
        return v
    }

    func space(_ args: [String: Any]) throws -> AppSpace {
        let id = try string(args, "id")
        guard let s = model.spaces.first(where: { $0.id == id }) else { throw Failure.notFound("no Space \(id)") }
        return s
    }

    static func rect(_ r: [String: Any]) -> CGRect? {
        func n(_ k: String) -> CGFloat? { (r[k] as? NSNumber).map { CGFloat(truncating: $0) } }
        guard let x = n("x"), let y = n("y"), let w = n("width"), let h = n("height") else { return nil }
        return CGRect(x: x, y: y, width: w, height: h)
    }

    // MARK: - Events

    /// Tells the page when what it shows changed (it asks again).
    func startEvents() {
        followStartup()
        follow("spaces.changed") { [weak self, model] in
            _ = (model.spaces, model.selectedSpaceId, model.loaded, model.rosterError)
            self?.pruneStreams()
        }
        follow("keyvault.changed") { [model] in _ = (model.keyvault.overview, model.keyvault.busy, model.keyvault.dismissed) }
        follow("session.changed") { [model] in _ = (model.identity, model.signIn, model.cloudConfigured) }
        follow("settings.changed") { [model] in _ = model.settings }
        // These views are rewritten on every read, so the event goes out
        // only when what the page shows differs.
        followChanges("machines.changed") { [weak self] in
            guard let self else { return "" }
            return Self.signature([self.hostStatus(), BridgeValue.encode(self.model.devices.view)])
        }
        followChanges("agents.changed") { [model] in
            Self.signature([model.persistent.agentsInput["agents"] ?? NSNull(),
                            BridgeValue.encode(model.agentRows), model.agentsPending])
        }
    }

    private func followChanges(_ event: String, _ read: @escaping @MainActor () -> String) {
        let now = withObservationTracking(read) { [weak self] in
            DispatchQueue.main.async { self?.followChanges(event, read) }
        }
        if let last = signatures[event], last != now { host?.emit(event) }
        signatures[event] = now
    }

    static func signature(_ value: Any) -> String {
        (try? JSONSerialization.data(withJSONObject: value, options: [.sortedKeys, .fragmentsAllowed]))
            .map { String(decoding: $0, as: UTF8.self) } ?? ""
    }

    private func follow(_ event: String, _ read: @escaping @MainActor () -> Void) {
        withObservationTracking(read) { [weak self] in
            DispatchQueue.main.async {
                guard let self else { return }
                self.host?.emit(event)
                self.follow(event, read)
            }
        }
    }
}

extension NSColor {
    /// `#rrggbb` (or `rrggbb`).
    convenience init?(webHex: String) {
        var s = webHex.trimmingCharacters(in: .whitespaces)
        if s.hasPrefix("#") { s.removeFirst() }
        guard s.count == 6, let v = UInt32(s, radix: 16) else { return nil }
        self.init(srgbRed: CGFloat((v >> 16) & 0xFF) / 255, green: CGFloat((v >> 8) & 0xFF) / 255,
                  blue: CGFloat(v & 0xFF) / 255, alpha: 1)
    }
}
