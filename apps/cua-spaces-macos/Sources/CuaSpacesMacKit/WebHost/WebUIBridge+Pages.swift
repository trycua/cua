// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
import CuaSpacesStreaming
import Foundation

/// The web UI's pages beyond Spaces, the Keyvault and Agents: New Space's
/// address form and "Connect a cloud", Teleport and Share, Cua Volume,
/// Settings (About, launch at login, Devices, Storage), Notifications, usage
/// events, a Space's detail and This machine.
///
/// Every method keeps the web bridge's operation name and arguments, and
/// runs what the SwiftUI screen for it runs: the same view model, SDK call
/// or daemon tool. Nothing here is a new mechanism. Teleport, sharing and
/// the Keyvault go only to the services the native sheets call, and the
/// daemon asks for presence where the native path does.
extension WebUIBridge {
    // swiftlint:disable:next cyclomatic_complexity
    func routePage(_ method: String, _ args: [String: Any]) async throws -> Any? {
        switch method {
        // New Space (the wizard itself stays in the native sheet)
        case "spaces.add": return try await addByAddress(args)
        case "clouds.status": return try await cloudTool("cloud_status", [:])
        case "clouds.test": return try await cloudTool("cloud_test", try object(args, "target"))
        case "clouds.connect":
            var target = try object(args, "target")
            target["make_default"] = args["makeDefault"] as? Bool ?? false
            let row = try await cloudTool("cloud_connect", target)
            await model.cloud.refresh()
            return row

        // Teleport and Share
        case "teleport.catalog", "teleport.entryForPath", "teleport.windows", "teleport.remoteWindows",
             "teleport.icon", "teleport.thumbnail", "teleport.plan", "teleport.run", "teleport.sites", "teleport.remembered",
             "teleport.streamWindow":
            return try await teleport(method, args)
        case "sharing.list":
            return BridgeValue.encode(try await model.backend.shares(id: try spaceId(args)))
        case "sharing.share":
            return BridgeValue.encode(try await model.backend.share(
                id: try spaceId(args), who: try string(args, "who"), role: try string(args, "role")))
        case "sharing.unshare":
            return BridgeValue.encode(try await model.backend.unshare(id: try spaceId(args), who: try string(args, "who")))

        // Settings, Agents: the daemon's agent_keys methods (the key travels
        // once, in agentKeys.set's arguments; answers never carry one).
        case "agentKeys.list": return try await model.agentKeys.call("agent_keys.list")
        case "agentKeys.set":
            var keyArgs: [String: Any] = ["provider": try string(args, "provider"), "value": try string(args, "value")]
            if let env = args["env"] as? String, !env.isEmpty { keyArgs["env"] = env }
            return try await model.agentKeys.call("agent_keys.set", keyArgs)
        case "agentKeys.remove": return try await model.agentKeys.call("agent_keys.remove", ["env": try string(args, "env")])

        // Cua Volume
        case "volume.overview": return try await volumeOverview()
        case "volume.storage": return (try? await tool("volume_storage")) ?? NSNull()
        case "volume.storageSet": return try await tool("volume_storage_set", try object(args, "update"))
        case "volume.mount": return try await tool("volume_mount")
        case "volume.unmount": return try await tool("volume_unmount")
        case "volume.approve": _ = try await tool("volume_approve", ["request_id": try string(args, "id")]); return NSNull()
        case "volume.deny": _ = try await tool("volume_deny", ["request_id": try string(args, "id")]); return NSNull()
        case "volume.revoke": _ = try await tool("volume_revoke", ["grant_id": try string(args, "id")]); return NSNull()
        case "volume.resolve": _ = try await tool("volume_sync_resolve", ["path": try string(args, "path")]); return NSNull()
        case "volume.reveal":
            model.persistent.reveal(try homePath(args))
            return NSNull()
        case "agents.setupDriver":
            guard let setup = model.agentSetup else { throw Failure.unsupported("Agent setup is not available in this build") }
            return BridgeValue.encode(try await setup.setUpCuaDriver(agents: try strings(args, "agents")))

        // Settings, About
        case "about.get": return BridgeValue.encode(model.updates.input)
        case "about.set":
            let updates = model.updates
            guard updates.updater != nil else { throw Failure.unsupported("Updates are off in this build") }
            if let on = args["autoCheck"] as? Bool { updates.setAutoCheck(on) }
            if let on = args["autoInstall"] as? Bool { updates.setAutoInstall(on) }
            if let channel = args["channel"] as? String { updates.choose(channel: channel) }
            return BridgeValue.encode(updates.input)
        case "about.checkNow":
            guard model.updates.updater != nil else { throw Failure.unsupported("Updates are off in this build") }
            // Sparkle's result is a modal alert: start the check after this
            // answer, so the page never waits on someone closing it.
            DispatchQueue.main.async { [model] in model.updates.checkNow() }
            var input = BridgeValue.encode(model.updates.input) as? [String: Any] ?? [:]
            input["checking"] = true
            return input

        // Settings, launch at login
        case "loginItem.get":
            model.readLoginItem()
            return try loginItem()
        case "loginItem.set":
            guard let on = args["on"] as? Bool else { throw Failure.badArgs("on: boolean") }
            model.setLaunchAtLogin(on)
            if let error = model.loginItemError { throw Failure(code: "failed", message: error) }
            return try loginItem()
        case "loginItem.openSettings":
            await model.press(row: "launch-at-login-approve")
            return NSNull()

        // Settings, Devices
        case "devices.get", "devices.enroll", "devices.checkEnrolled", "devices.approve", "devices.rename",
             "devices.revoke", "devices.confirmMachine":
            return try await devices(method, args)

        // Settings, Storage
        case "storage.get":
            let storage = model.storage
            await storage.load()
            return ["os": "macos", "home": userHome(), "storage": storage.storage ?? NSNull(),
                    "mount": storage.mount ?? NSNull(), "cache": storage.cache ?? NSNull()] as [String: Any]
        case "storage.run": return try await storageRun(try object(args, "request"))

        // Notifications
        case "notifications.list":
            guard model.persistent.canCallTools else { throw Failure.unsupported("Notifications need the cua daemon") }
            await model.persistent.pollNotifications()
            return BridgeValue.encode(model.persistent.feed)
        case "notifications.markAllRead":
            guard model.persistent.canCallTools else { throw Failure.unsupported("Notifications need the cua daemon") }
            await model.persistent.markAllRead()
            return NSNull()

        // Usage events
        case "telemetry.track":
            guard let raw = args["signals"] as? [Any] else { throw Failure.badArgs("signals: array") }
            // The page's switch is the app's: off (or locked off) drops every
            // signal here, before the SDK's own check.
            guard let telemetry = model.telemetrySink, telemetry.status().enabled else { return NSNull() }
            let signals = raw.compactMap(Self.telemetrySignal)
            if !signals.isEmpty { telemetry.record(signals) }
            return NSNull()

        // A Space's detail
        case "spaces.usage":
            return BridgeValue.encode(await model.backend.usage(id: try spaceId(args)))
        case "spaces.windows":
            let stream = try await stream(for: try spaceId(args))
            await stream.rows.refresh()
            // The open panels too: one closed by its own button shows on the next read.
            return ["windows": BridgeValue.encode((stream.rows.windows ?? []).map(StreamRowsModel.remote)),
                    "display": BridgeValue.encode(stream.rows.display),
                    "open": stream.rows.openRows(openKeys: stream.pips.openKeys)] as [String: Any]
        case "stream.pip":
            let stream = try await stream(for: try spaceId(args))
            let command = try object(args, "command")
            guard let type = command["type"] as? String, let row = command["row"] as? String else {
                throw Failure.badArgs("command: {type, row}")
            }
            if row != appStreamDesktopRowId(), stream.rows.window(id: row) == nil { await stream.rows.refresh() }
            let source: StreamSource
            if row == appStreamDesktopRowId() {
                source = .desktop
            } else if let w = stream.rows.window(id: row) {
                source = .window(w)
            } else {
                throw Failure.notFound("no window \(row)")
            }
            if type == "open" { stream.pips.popOut(source) } else { stream.pips.popIn(source) }
            return stream.rows.openRows(openKeys: stream.pips.openKeys)

        // This machine
        case "host.setUp":
            guard let host = model.host.host else { throw Failure.unsupported("This Mac can't be set up for access in this build") }
            let request = try hostRequest(try object(args, "request"))
            do {
                try await model.host.setUp(request, on: host)
            } catch {
                // What the native form shows: a short title, what to do,
                // and the raw error under Details.
                throw Self.hostFailure(HostModel.isUnauthenticated(error)
                    ? HostSetupFailure.presenting(LiveSpacesBackend.words(error), as: .signedOut)
                    : HostSetupFailure.presenting(LiveSpacesBackend.words(error)))
            }
            return hostStatus()
        case "host.action":
            guard model.host.host != nil else { throw Failure.unsupported("This Mac can't be set up for access in this build") }
            let id = try hostAction(try string(args, "action"))
            await model.host.run(id)
            if let failure = model.host.actionFailure { throw Self.hostFailure(failure) }
            return hostStatus()
        case "host.openSettings":
            let url = try string(args, "url")
            guard url.hasPrefix("x-apple.systempreferences:"), let u = URL(string: url) else {
                throw Failure.badArgs("Only System Settings panes open here")
            }
            NSWorkspace.shared.open(u)
            return NSNull()

        default:
            return nil
        }
    }

    // MARK: - Arguments

    func spaceId(_ args: [String: Any]) throws -> String { try string(args, "spaceId") }

    func object(_ args: [String: Any], _ key: String) throws -> [String: Any] {
        guard let v = args[key] as? [String: Any] else { throw Failure.badArgs("\(key): object") }
        return v
    }

    /// A path in the user's home (what Show in Finder may reveal).
    func homePath(_ args: [String: Any]) throws -> String {
        let full = (homeExpanded(try string(args, "path")) as NSString).standardizingPath
        let home = (userHome() as NSString).standardizingPath
        guard full == home || full.hasPrefix(home + "/") else { throw Failure.badArgs("path: in your home folder") }
        return full
    }

    // MARK: - The daemon's tools

    /// One of the daemon's Spaces tools the native pages run.
    func tool(_ name: String, _ args: [String: Any] = [:]) async throws -> Any {
        guard model.persistent.canCallTools else { throw Failure.unsupported("This needs the cua daemon") }
        return try await model.persistent.daemonTool(name, args)
    }

    func cloudTool(_ name: String, _ args: [String: Any]) async throws -> Any {
        guard let clouds = model.backend as? CloudToolRunning else { throw Failure.unsupported("Clouds need the cua daemon") }
        return try await clouds.cloudTool(name, args)
    }

    func volumeOverview() async throws -> Any {
        guard model.persistent.canCallTools else { throw Failure.unsupported("Cua Volume needs the cua daemon") }
        func list(_ v: Any?, _ key: String) -> Any { (v as? [String: Any])?[key] as? [Any] ?? [] }
        async let requests = try? tool("volume_requests")
        async let grants = try? tool("volume_grants")
        async let mount = try? tool("volume_mount_status")
        async let sync = try? tool("volume_sync_status")
        let (r, g, m, s) = await (requests, grants, mount, sync)
        return ["os": "macos", "home": userHome(), "requests": list(r, "requests"), "grants": list(g, "grants"),
                "mount": (m as? [String: Any]) ?? NSNull(), "sync": (s as? [String: Any]) ?? NSNull()] as [String: Any]
    }

    /// Settings, Storage: the command the core's section asked for, as
    /// `StorageModel.send` runs it; then the section reads again.
    func storageRun(_ request: [String: Any]) async throws -> Any {
        let storage = model.storage
        defer { Task { await storage.load() } }
        switch request["kind"] as? String ?? "" {
        case "test", "save", "adopt":
            return try await tool("volume_storage_set", try object(request, "update"))
        case "mount": _ = try await tool("volume_mount")
        case "unmount": _ = try await tool("volume_unmount")
        case "reveal": storage.reveal(try homePath(request))
        case "open-url":
            let url = try string(request, "url")
            guard url.hasPrefix("x-apple.systempreferences:"), let u = URL(string: url) else {
                throw Failure.badArgs("Only System Settings panes open here")
            }
            storage.openURL(u)
        case "set-cache":
            guard let bytes = request["capacity_bytes"] as? NSNumber else { throw Failure.badArgs("capacity_bytes: number") }
            _ = try await tool("volume_cache_set", ["capacity_bytes": bytes])
        case "clear-cache": _ = try await tool("volume_cache_clear")
        default: throw Failure.badArgs("request.kind")
        }
        return NSNull()
    }

    // MARK: - New Space

    /// "Connect by address": the SDK's handshake, then the row it added.
    func addByAddress(_ args: [String: Any]) async throws -> Any {
        let before = Set(model.spaces.map(\.id))
        let token = (args["token"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        let name = (args["name"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        try await model.addByAddress(url: try string(args, "url"), token: token, name: name)
        guard let added = model.spaces.first(where: { !before.contains($0.id) }) else {
            throw Failure(code: "failed", message: "The Space was added but is not listed yet")
        }
        return BridgeValue.encode(added)
    }

    // MARK: - Settings

    func loginItem() throws -> [String: Any] {
        guard let input = model.loginItemInput else { throw Failure.unsupported("Launch at login is not available in this build") }
        return ["status": BridgeValue.encode(input.status), "providesSpaces": input.providesSpaces,
                "runsAgents": input.runsAgents]
    }

    /// Settings, Devices: the relay's calls `DevicesModel` makes. Approving
    /// asks for presence first, as the native sheet does.
    func devices(_ method: String, _ args: [String: Any]) async throws -> Any {
        let devices = model.devices
        guard let relay = devices.devices else { throw Failure.unsupported("Devices need a signed-in Cua account") }
        switch method {
        case "devices.get":
            // As Settings → Devices: a relay that refuses this device (not
            // enrolled) still shows the page, "Needs enrollment" and Enroll…,
            // with the relay's words under it.
            await devices.refresh()
            var input = BridgeValue.encode(devices.input) as? [String: Any] ?? [:]
            input["readError"] = devices.error ?? NSNull()
            // What the native page names this Mac before the relay does.
            input["deviceName"] = Foundation.Host.current().localizedName ?? NSNull()
            return input
        case "devices.enroll":
            let r = try await relay.enroll()
            devices.pendingCode = r.enrolled ? nil : r.code
            return ["enrolled": r.enrolled, "code": r.code ?? NSNull()] as [String: Any]
        case "devices.checkEnrolled":
            let ok = await relay.checkEnrolled()
            if ok { devices.pendingCode = nil }
            return ok
        case "devices.approve":
            let code = (args["code"] as? String).flatMap { $0.isEmpty ? nil : $0 }
            let deviceId = (args["deviceId"] as? String).flatMap { $0.isEmpty ? nil : $0 }
            guard code != nil || deviceId != nil else { throw Failure.badArgs("code or deviceId") }
            let name = deviceId.flatMap { id in devices.input.devices.first { $0.id == id }?.name } ?? "this device"
            try await devices.presence.confirm(reason: "approve \u{201c}\(name)\u{201d} for your Cua account")
            try await relay.approve(code: code, deviceId: deviceId)
            await devices.refresh()
            return NSNull()
        case "devices.rename":
            guard let clean = appDevicesCleanName(name: try string(args, "name")) else { throw Failure.badArgs("name") }
            try await relay.rename(id: try string(args, "id"), name: clean)
        case "devices.revoke":
            try await relay.revoke(id: try string(args, "id"))
        case "devices.confirmMachine":
            try await relay.confirmMachine(id: try string(args, "id"))
        default:
            throw Failure.unimplemented(method)
        }
        await devices.refresh()
        return NSNull()
    }

    /// A usage event as the core's signal: known kinds with every field a
    /// fixed word, flag or duration (anything else is dropped whole, so a
    /// name or a path never reaches the telemetry).
    static func telemetrySignal(_ raw: Any) -> AppTelemetrySignal? {
        guard let s = raw as? [String: Any], let type = s["type"] as? String else { return nil }
        func word(_ k: String) -> String? {
            guard let v = s[k] as? String, (1...64).contains(v.count),
                  v.allSatisfy({ $0.isASCII && ($0.isLowercase || $0.isNumber || $0 == "_") }) else { return nil }
            return v
        }
        func flag(_ k: String) -> Bool? {
            guard let n = s[k] as? NSNumber, CFGetTypeID(n) == CFBooleanGetTypeID() else { return nil }
            return n.boolValue
        }
        func ms(_ k: String) -> UInt64? {
            guard let n = s[k] as? NSNumber, CFGetTypeID(n) != CFBooleanGetTypeID(), n.doubleValue >= 0,
                  n.doubleValue == n.doubleValue.rounded() else { return nil }
            return n.uint64Value
        }
        /// An error enum case (`InsufficientDisk`), or empty when the shell has none.
        func variant(_ k: String) -> String? {
            let v = s[k] as? String ?? ""
            guard v.count <= 64, v.allSatisfy({ $0.isASCII && ($0.isLetter || $0.isNumber || $0 == "_") }) else { return nil }
            return v
        }
        switch type {
        case "feature":
            guard let f = word("feature") else { return nil }
            return .feature(feature: f)
        case "step":
            guard let step = word("step"), let ok = flag("ok") else { return nil }
            return .step(step: step, ok: ok)
        case "sign-in-failed":
            guard let kind = word("errorKind") else { return nil }
            return .signInFailed(errorKind: kind)
        case "launched":
            if s["onboardingEligible"] is NSNull || s["onboardingEligible"] == nil { return .launched(onboardingEligible: nil) }
            guard let eligible = flag("onboardingEligible") else { return nil }
            return .launched(onboardingEligible: eligible)
        case "onboarding-page":
            guard let page = word("page"), let action = word("action"), let choice = word("choice") else { return nil }
            return .onboardingPage(page: page, action: action, choice: choice)
        case "space-wizard":
            guard let action = word("action") else { return nil }
            return .spaceWizard(action: action)
        case "space-create":
            guard let location = word("location"), let os = word("guestOs"), let kind = word("kind"),
                  let outcome = word("outcome"), let phase = word("failedPhase"), let stalled = flag("stalled"),
                  let elapsed = ms("elapsedMs"), let gpu = flag("gpu"), let errorVariant = variant("errorVariant") else { return nil }
            return .spaceCreate(location: location, guestOs: os, kind: kind, outcome: outcome, failedPhase: phase,
                                stalled: stalled, elapsedMs: elapsed, gpu: gpu, errorVariant: errorVariant)
        case "space-create-started":
            guard let location = word("location"), let os = word("guestOs"), let kind = word("kind"),
                  let gpu = flag("gpu") else { return nil }
            return .spaceCreateStarted(location: location, guestOs: os, kind: kind, gpu: gpu)
        case "volume-setup":
            guard let surface = word("surface"), let storage = word("storage"), let finder = flag("addToFinder"),
                  let method = word("mountMethod"), let outcome = word("outcome") else { return nil }
            return .volumeSetup(surface: surface, storage: storage, addToFinder: finder, mountMethod: method, outcome: outcome)
        case "share":
            guard let action = word("action"), let role = word("role"), let outcome = word("outcome") else { return nil }
            return .share(action: action, role: role, outcome: outcome)
        case "app-update":
            guard let action = word("action"), let channel = word("channel"), let trigger = word("trigger") else { return nil }
            return .appUpdate(action: action, channel: channel, trigger: trigger)
        case "device-enroll":
            guard let method = word("method"), let outcome = word("outcome") else { return nil }
            return .deviceEnroll(method: method, outcome: outcome)
        case "experiment":
            guard let action = word("action"), let experiment = word("experiment") else { return nil }
            return .experiment(action: action, experiment: experiment)
        case "experiments-on":
            guard let list = s["experiments"] as? [Any], list.count <= 16 else { return nil }
            let words = list.compactMap { v -> String? in
                guard let w = v as? String, (1...64).contains(w.count),
                      w.allSatisfy({ $0.isASCII && ($0.isLowercase || $0.isNumber || $0 == "_") }) else { return nil }
                return w
            }
            guard words.count == list.count else { return nil }
            return .experimentsOn(experiments: words)
        default:
            return nil
        }
    }

    // MARK: - This machine

    func hostRequest(_ r: [String: Any]) throws -> AppHostSetupRequest {
        guard let mode = r["mode"] as? String, !mode.isEmpty else { throw Failure.badArgs("request.mode") }
        func text(_ k: String) -> String? { (r[k] as? String).flatMap { $0.isEmpty ? nil : $0 } }
        return AppHostSetupRequest(mode: mode, relayUrl: text("relayUrl"), direct: text("direct"), name: text("name"),
                                   allow: r["allow"] as? [String], profile: text("profile"),
                                   shareDesktop: r["shareDesktop"] as? Bool, provideSpaces: r["provideSpaces"] as? Bool)
    }

    /// A failed setup or button as the page shows it: the plain message,
    /// with the title, the raw error and the button's label alongside.
    static func hostFailure(_ f: HostSetupFailure) -> Failure {
        Failure(code: "failed", message: f.message, presented: f)
    }

    func hostAction(_ id: String) throws -> AppHostActionId {
        switch id {
        case "sign-in": return .signIn
        case "stop-sharing": return .stopSharing
        case "resume-sharing": return .resumeSharing
        case "remove": return .remove
        case "share-desktop": return .shareDesktop
        case "hide-desktop": return .hideDesktop
        case "provide-spaces": return .provideSpaces
        case "stop-providing-spaces": return .stopProvidingSpaces
        default: throw Failure.badArgs("\(id) is not a host action")
        }
    }

    // MARK: - A Space's stream

    /// The Space's stream rows and panels, opened once per Space (its
    /// detail view's: the backend's stream provider).
    func stream(for id: String) async throws -> (rows: StreamRowsModel, pips: StreamPiPSet) {
        if let s = streams[id] { return s }
        guard let space = model.spaces.first(where: { $0.id == id }) else { throw Failure.notFound("no Space \(id)") }
        let provider = try await model.backend.streamProvider(id: id)
        let s = (rows: StreamRowsModel(space: space, provider: provider, backend: model.backend),
                 pips: StreamPiPSet(provider: provider))
        streams[id] = s
        return s
    }

    /// Closes and drops the streams of Spaces that are gone (a loaded
    /// roster only: an empty one is a launch, not a deletion).
    func pruneStreams() {
        guard model.loaded else { return }
        let ids = Set(model.spaces.map(\.id))
        for id in streams.keys where !ids.contains(id) {
            streams[id]?.pips.popInAll()
            streams[id] = nil
        }
    }

    // MARK: - Teleport

    func teleportContext(_ args: [String: Any]) async throws -> (CuaSpacesFFI.Teleport, CuaSDK.Space) {
        let id = try spaceId(args)
        guard let context = try await model.backend.teleportContext(id: id) else {
            throw Failure.unsupported("Teleport needs a reachable Space")
        }
        return context
    }

    /// "Teleport an app": the SDK calls `TeleportModel` and its picker
    /// sources make. The SDK re-checks the consent a run carries.
    func teleport(_ method: String, _ args: [String: Any]) async throws -> Any {
        switch method {
        case "teleport.catalog":
            let (teleport, space) = try await teleportContext(args)
            let entries = try await teleport.catalog(options: try await teleport.spaceHint(space: space))
            // The latest read replaces the last (an app picked from a window
            // joins them in `teleport.entryForPath`).
            teleportEntries = Dictionary(entries.map { ($0.id, $0) }, uniquingKeysWith: { _, last in last })
            return entries.map { BridgeValue.encode(appCatalogEntry(entry: $0)) }
        case "teleport.entryForPath":
            guard let teleport = model.backend.teleportHandle() else { throw Failure.unsupported("Teleport is not available") }
            // Reads the app's bundle, which may sit in a folder macOS asks
            // about (Downloads, a disk image): never on the main thread.
            let path = try string(args, "path")
            let entry = try await Task.detached { try teleport.catalogEntryForPath(path: path, options: nil) }.value
            teleportEntries[entry.id] = entry
            return BridgeValue.encode(appCatalogEntry(entry: entry))
        case "teleport.windows":
            let sources = TeleportPickerSources.live(teleport: model.backend.teleportHandle(), space: nil)
            return BridgeValue.encode(await sources.openWindows())
        case "teleport.remoteWindows":
            let (teleport, space) = try await teleportContext(args)
            return BridgeValue.encode(await TeleportPickerSources.live(teleport: teleport, space: space).remoteWindows())
        case "teleport.icon":
            let icon = try object(args, "icon")
            switch icon["kind"] as? String {
            case "host":
                let sources = TeleportPickerSources.live(teleport: model.backend.teleportHandle(), space: nil)
                return Self.dataURL(await sources.hostIcon(try string(icon, "path")))
            case "guest":
                let request = SpaceAppIconRequest(appName: icon["appName"] as? String ?? "", appId: icon["appId"] as? String ?? "",
                                                  pid: (icon["pid"] as? NSNumber)?.uint32Value ?? 0)
                return Self.dataURL(await model.backend.appIcons(id: try spaceId(args), requests: [request]).first ?? nil)
            default:
                return NSNull()
            }
        case "teleport.thumbnail":
            let t = try object(args, "thumbnail")
            switch t["kind"] as? String {
            case "host-window":
                guard let id = (t["windowId"] as? NSNumber)?.uint32Value else { throw Failure.badArgs("windowId") }
                let sources = TeleportPickerSources.live(teleport: model.backend.teleportHandle(), space: nil)
                return Self.dataURL(await sources.hostThumbnail(id))
            case "guest-window":
                let (teleport, space) = try await teleportContext(args)
                let epoch = (t["epoch"] as? NSNumber)?.uint64Value ?? 0
                let sources = TeleportPickerSources.live(teleport: teleport, space: space)
                return Self.dataURL(await sources.guestThumbnail(try string(t, "windowId"), epoch))
            default:
                return NSNull()
            }
        case "teleport.plan":
            let (teleport, space) = try await teleportContext(args)
            let entry = try object(args, "entry")
            guard let sdk = teleportEntries[try string(entry, "id")] else { throw Failure.notFound("Read the catalog again") }
            let move: TeleportMove
            switch try string(args, "move") {
            case "app_with_files": move = .appWithFiles
            case "app_with_state": move = .appWithState
            default: move = .appOnly
            }
            let groups: [TeleportSensitiveGroup] = (args["sensitiveGroups"] as? [String] ?? []).compactMap {
                switch $0 {
                case "sign_ins": return .signIns
                case "passwords": return .passwords
                case "history": return .history
                default: return nil
                }
            }
            let plan = try await teleport.plan(app: sdk, space: space, options: TeleportPlanOptions(
                moves: move, files: args["files"] as? [String] ?? [], stateItems: nil,
                sensitiveGroups: groups, scope: nil, launch: true))
            let view = appTeleportPlan(plan: plan)
            teleportPlans[try spaceId(args)] = (view.json, plan, view.app.providerId)
            return BridgeValue.encode(view)
        case "teleport.run":
            let (teleport, space) = try await teleportContext(args)
            let planArg = try object(args, "plan")
            let planKey = try spaceId(args)
            guard let kept = teleportPlans[planKey], kept.json == (try string(planArg, "json")) else {
                throw Failure.notFound("Plan the teleport again")
            }
            let plan = kept.plan
            // Finished, failed or cancelled: the plan is spent.
            defer { if teleportPlans[planKey]?.json == kept.json { teleportPlans[planKey] = nil } }
            let runId = try string(args, "runId")
            let consentArgs = try object(args, "consent")
            let consent = Self.teleportConsent(consentArgs)
            // Next time the review starts from the sites sent now, and the
            // notch shows the transfer while it runs, as the native sheet does.
            rememberChoice(providerId: kept.providerId, spaceId: planKey, consent: consentArgs)
            setTransfer(true)
            defer { setTransfer(false) }
            let report = try await teleport.run(plan: plan, space: space, consent: consent, listener: RunEvents { [weak self] event in
                Task { @MainActor in
                    self?.host?.emit("teleport.progress", payload: [
                        "runId": runId, "event": BridgeValue.encode(appTeleportRunEvent(event: event)),
                    ])
                }
            })
            return Self.runReport(report)
        case "teleport.sites":
            // Counts per site, never values; the daemon asks for Touch ID
            // before it shows names, as for the native review.
            guard let client = model.keyvault.client else { throw Failure.unsupported("The Keyvault is not available") }
            return BridgeValue.encode(try await client.inventory(app: try string(args, "providerId"), profile: nil))
        case "teleport.remembered":
            let key = appReviewRememberKey(app: try string(args, "providerId"), space: spaceName(try string(args, "spaceId")))
            return appReviewRemembered(choices: model.settings.teleportChoices, key: key).map { $0 as Any } ?? NSNull()
        case "teleport.streamWindow":
            let stream = try await stream(for: try spaceId(args))
            let id = try string(args, "windowId")
            if stream.rows.window(id: id) == nil { await stream.rows.refresh() }
            guard let window = stream.rows.window(id: id) else { throw Failure.notFound("no window \(id)") }
            stream.pips.popOut(.window(window))
            return NSNull()
        default:
            throw Failure.unimplemented(method)
        }
    }

    /// A Space's name, which the remembered choices are keyed by (the
    /// native review's `rememberKey`).
    func spaceName(_ id: String) -> String { model.spaces.first { $0.id == id }?.name ?? id }

    /// Remembers the sites sent now (to this Space, from this app) for the
    /// next review, as `TeleportModel.rememberChoice` does: only a choice of
    /// sites from the live app, never the Keyvault as the source.
    func rememberChoice(providerId: String?, spaceId: String, consent: [String: Any]) {
        guard let providerId, let domains = consent["cookieDomains"] as? [String], consent["fromVault"] as? [String] == nil else { return }
        let key = appReviewRememberKey(app: providerId, space: spaceName(spaceId))
        model.settings.teleportChoices = appReviewRemember(choices: model.settings.teleportChoices, key: key, domains: domains)
        model.saveSettings()
    }

    /// The notch's transfer indicator while a teleport runs
    /// (`SpaceDetailView.openTeleport`'s `onTransfer`).
    func setTransfer(_ running: Bool) {
        let notch = model.notch
        notch.setActivity(hotspot: notch.state.hotspot, transfer: running ? AppNotchTransfer(sent: nil, total: nil) : nil)
    }

    /// The consent the page sends, as the SDK's: the core's consent record
    /// through `sdkConsent`, the same path `TeleportModel.confirm` takes, so
    /// every field (Save to Keyvault included) reaches the run.
    static func teleportConsent(_ c: [String: Any]) -> TeleportConsent {
        sdkConsent(AppTeleportConsent(
            approved: c["approved"] as? Bool ?? false,
            acknowledgeSensitive: c["acknowledgeSensitive"] as? Bool ?? false,
            saveToKeyvault: c["saveToKeyvault"] as? Bool ?? false,
            acknowledgeRelayPlaintext: c["acknowledgeRelayPlaintext"] as? Bool ?? false,
            cookieDomains: c["cookieDomains"] as? [String],
            exclude: c["exclude"] as? [String] ?? [],
            fromVault: c["fromVault"] as? [String],
            includePasswords: c["includePasswords"] as? Bool ?? false))
    }

    /// A run's report, as the page reads it (`RunReport`).
    static func runReport(_ report: TeleportRunReport) -> [String: Any] {
        ["appId": report.appId, "installed": report.installed, "sent": report.sent,
         "imported": report.imported, "skipped": report.skipped, "launched": report.launched]
    }

    /// Icon or preview bytes as a `data:` URL (PNG, JPEG or SVG).
    static func dataURL(_ data: Data?) -> Any {
        guard let data, !data.isEmpty else { return NSNull() }
        let type: String
        if data.starts(with: [0x89, 0x50, 0x4E, 0x47]) { type = "image/png" }
        else if data.starts(with: [0xFF, 0xD8]) { type = "image/jpeg" }
        else if data.first == UInt8(ascii: "<") { type = "image/svg+xml" }
        else { type = "application/octet-stream" }
        return "data:\(type);base64,\(data.base64EncodedString())"
    }
}
