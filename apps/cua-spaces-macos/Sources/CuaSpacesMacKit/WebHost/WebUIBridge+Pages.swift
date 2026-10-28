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
        case "clouds.disconnect":
            let receipt = try await cloudTool("cloud_disconnect",
                                              ["provider": try string(args, "provider"),
                                               "delete": args["deleteResources"] as? Bool ?? false])
            await model.cloud.refresh()
            return receipt
        case "clouds.approve":
            try await model.approveCloud(try string(args, "provider"), approve: args["approve"] as? Bool ?? false)
            return NSNull()

        // Teleport and Share
        case "teleport.catalog", "teleport.entryForPath", "teleport.windows", "teleport.remoteWindows",
             "teleport.icon", "teleport.thumbnail", "teleport.plan", "teleport.run", "teleport.sites", "teleport.remembered",
             "teleport.streamWindow":
            return try await teleport(method, args)
        // Links to things you run (`cua share`): the daemon's shares.
        case "links.list":
            return ["links": try await model.backend.links().map { BridgeValue.encode($0) }] as [String: Any]
        case "links.create":
            let ttl = (args["ttlSeconds"] as? NSNumber).map { $0.uint32Value }
            return BridgeValue.encode(try await model.backend.createLink(
                target: try string(args, "target"), access: args["access"] as? String, ttlSeconds: ttl, name: args["name"] as? String))
        case "links.stop":
            return ["stopped": try await model.backend.stopLink(id: try string(args, "id"))] as [String: Any]
        // The Space thread (CUA-2155): the daemon's tools, answered as-is;
        // the page maps them (`ops/thread.ts`).
        case "thread.read", "thread.post", "thread.members", "thread.setRights",
             "thread.agentAdd", "thread.agentRemove",
             "thread.joinCodeCreate", "thread.joinCodeList", "thread.joinCodeCancel", "thread.settings":
            return try await threadTool(method, args)
        case "sharing.list":
            return BridgeValue.encode(try await model.backend.shares(id: try spaceId(args)))
        case "sharing.share":
            return BridgeValue.encode(try await model.backend.share(
                id: try spaceId(args), who: try string(args, "who"), role: try string(args, "role")))
        case "sharing.unshare":
            return BridgeValue.encode(try await model.backend.unshare(id: try spaceId(args), who: try string(args, "who")))

        // Sharing is for teams: who lost access, and "seen" (empty when signed
        // out or from a server that predates it).
        case "sharing.removals":
            guard model.identity != nil, let reader = model.teamsReader else { return [] as [Any] }
            return BridgeValue.encode(try await reader.shareRemovals())
        case "sharing.ackRemovals":
            if model.identity != nil { try await model.teamsReader?.ackShareRemovals() }
            return NSNull()

        // Settings, Connectors: the daemon's agent_keys methods (the key travels
        // once, in agentKeys.set's arguments; answers never carry one).
        case "agentKeys.list": return try await model.agentKeys.call("agent_keys.list")
        case "agentKeys.set":
            var keyArgs: [String: Any] = ["provider": try string(args, "provider"), "value": try string(args, "value")]
            if let env = args["env"] as? String, !env.isEmpty { keyArgs["env"] = env }
            return try await model.agentKeys.call("agent_keys.set", keyArgs)
        case "agentKeys.remove": return try await model.agentKeys.call("agent_keys.remove", ["env": try string(args, "env")])
        // Which provider keys your login shell has (names only), and one saved from it (the value never crosses the bridge).
        case "agentKeys.shell": return try await model.agentKeys.call("agent_keys.shell")
        case "agentKeys.fromShell": return try await model.agentKeys.call("agent_keys.from_shell", ["env": try string(args, "env")])

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

        // The in-app update indicator: Sparkle downloads in the background,
        // and the page asks to restart.
        case "update.get": return model.updates.updateState.json
        case "update.install":
            guard model.updates.updater != nil else { throw Failure.unsupported("Updates are off in this build") }
            // The app quits to install: answer first, then start.
            let answer = model.updates.updateState.json
            if model.updates.updateState.status == .downloaded {
                DispatchQueue.main.async { [model] in model.updates.installUpdate() }
            }
            return answer

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

        // "Add a computer" and "Share": the account's Teams (Settings, Teams)
        // and the current one.
        case "teams.list":
            await model.reloadTeams()
            var teams: [[String: Any]] = []
            if case .ready(let list) = model.teamsState {
                teams = list.map { ["id": $0.id, "alias": $0.alias, "name": $0.name, "role": $0.role, "plan": $0.plan] }
            }
            return ["teams": teams, "currentTeamId": model.settings.currentTeamId.map { $0 as Any } ?? NSNull()] as [String: Any]

        // Settings, Devices
        case "devices.get", "devices.enroll", "devices.checkEnrolled", "devices.approve", "devices.rename",
             "devices.revoke", "devices.confirmMachine", "devices.createPairingCode", "devices.pairingCodes",
             "devices.revokePairingCode":
            return try await devices(method, args)

        // Settings, Relay usage, and the notice past the allowance: the
        // same read as the native row (`app_relay_usage`). Signed out, or
        // a relay that meters nothing: null.
        case "relay.usage":
            let devices = model.devices
            guard devices.signedIn, let relay = devices.devices else { return NSNull() }
            guard let usage = try await relay.relayUsage(), usage.metered else { return NSNull() }
            return BridgeValue.encode(usage)

        // Processor, memory and disk for Home, the Spaces page and a
        // Space's page: the SDK's snapshot (every Space of yours, here and
        // on your other computers); This Mac's own numbers
        // (`HostResourceSampler`) while the SDK can't answer.
        case "resources.get":
            let visible = args["visible"] as? Bool ?? true
            if let snapshot = await model.backend.resources(visible: visible) {
                return resourcesJSON(snapshot)
            }
            return HostResourceSampler.shared.sample().snapshot()
        // The Machines page: rename a machine, Remove from account (what
        // each does to which machine is the core's `machines_view`).
        case "machines.rename":
            try await model.renameMachine(id: try string(args, "id"), name: try string(args, "name"))
            return NSNull()
        case "machines.remove":
            let machine = (args["machine"] as? String).flatMap { $0.isEmpty ? nil : $0 }
            let device = (args["device"] as? String).flatMap { $0.isEmpty ? nil : $0 }
            guard machine != nil || device != nil else { throw Failure.badArgs("machine or device") }
            try await model.removeMachine(machine: machine, device: device)
            return NSNull()
        // Each machine's own daemon checks its health; the next
        // `machines.list` view carries the issues.
        case "machines.checkHealth":
            await model.checkMachineHealth()
            return NSNull()

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
            return model.persistent.feedRows()
        case "notifications.markAllRead":
            guard model.persistent.canCallTools else { throw Failure.unsupported("Notifications need the cua daemon") }
            await model.persistent.markAllRead()
            return NSNull()
        case "notifications.markRead":
            guard model.persistent.canCallTools else { throw Failure.unsupported("Notifications need the cua daemon") }
            guard let ids = args["ids"] as? [String] else { throw Failure.badArgs("ids: array of strings") }
            await model.persistent.markRead(ids: ids)
            return NSNull()

        // Usage events
        case "telemetry.track":
            guard let raw = args["signals"] as? [Any] else { throw Failure.badArgs("signals: array") }
            // The page's switch is the app's: off (or locked off) drops every
            // signal here, before the SDK's own check.
            guard let telemetry = model.telemetrySink, telemetry.status().enabled else { return NSNull() }
            let signals = raw.compactMap(Self.telemetrySignal)
            // The page's first run moved past Welcome: its usage notice was
            // on screen, so events may be sent from here (this one included).
            if raw.contains(where: Self.leftWelcome) { telemetry.acknowledgeNotice() }
            if !signals.isEmpty { telemetry.record(signals) }
            return NSNull()

        // The one-time notice that usage data is now linked to the account
        // (CUA-1881); no telemetry (a bare model) has none.
        case "telemetry.linkNotice":
            guard let telemetry = model.telemetrySink else { return NSNull() }
            let n = telemetry.linkNotice()
            return ["due": n.due, "title": n.title, "body": n.body, "okLabel": n.okLabel,
                    "turnOffLabel": n.turnOffLabel, "detailsLabel": n.detailsLabel, "docsUrl": n.docsUrl] as [String: Any]
        case "telemetry.acknowledgeLinkNotice":
            model.telemetrySink?.acknowledgeLinkNotice()
            return NSNull()
        // The page's crash reporter (CUA-2343): its consent and build config.
        case "crashReporting.config":
            let status = model.telemetrySink?.status()
            return CrashReporting.bridgeConfig(
                info: Bundle.main.infoDictionary ?? [:],
                allowed: status.map { CrashReporting.isAllowed(enabled: $0.enabled, noticeShown: $0.noticeShown) } ?? false,
                env: ProcessInfo.processInfo.environment)

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
            if type == "open" {
                try await popOut(source, of: try spaceId(args), stream: stream)
            } else {
                stream.pips.popIn(source)
            }
            return stream.rows.openRows(openKeys: stream.pips.openKeys)

        // This machine
        case "host.setUp":
            guard let host = model.host.host else { throw Failure.unsupported("This Mac can't be set up for access in this build") }
            // A sign-in here already set it up (`addAfterSignIn`): wait for
            // that, and don't set it up twice.
            if let running = model.host.signInSetUp {
                await running.value
                if model.host.status?.configured == true { return hostStatus() }
            }
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
        case "host.joinWithCode":
            let relay = args["relay"] as? String
            guard let code = args["code"] as? String,
                  let cliArgs = JoinWithCode.arguments(code: code, relay: relay) else {
                throw Failure.badArgs("code: string")
            }
            guard let cua = AppEnvironment.bundledCuaPath else {
                throw Failure.unsupported("This build has no bundled cua to join with")
            }
            let was = model.host.providesSpaces
            do {
                try await JoinWithCode.run(cua: cua, args: cliArgs)
            } catch let failed as JoinWithCode.FailedRun {
                throw Failure(code: "failed", message: failed.message)
            }
            await model.host.refresh()
            model.host.followProviding(from: was)
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
            // Local Network: the daemon connects (which makes macOS ask, in a
            // dialog of its own); the pane opens only when it stays blocked
            // and no macOS prompt is waiting for an answer.
            if url.contains("Privacy_LocalNetwork") {
                await model.checkLocalNetworkFromDaemon(every: 0)
                if model.daemonLocalNetworkAllowed != true, LiveLocalNetworkProbe().promptsShowing() == 0 {
                    NSWorkspace.shared.open(u)
                }
                return NSNull()
            }
            // Screen Recording and Accessibility: the app's guide to the
            // pane (PermissionFlow), never macOS's bare prompt.
            if !PermissionGuide.guide(url) { NSWorkspace.shared.open(u) }
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

    /// `thread.read`, `thread.post`, `thread.members`, `thread.setRights`,
    /// `thread.agentAdd`, `thread.agentRemove`, `thread.joinCodeCreate`,
    /// `thread.joinCodeList`, `thread.joinCodeCancel`, `thread.settings`: the operation's
    /// arguments as the tool's (`threadReadArgs` in the web bridge), and the
    /// tool's JSON back untouched. A daemon (or SDK) without the thread
    /// answers `unsupported`, so the page keeps its classic layout.
    func threadTool(_ method: String, _ args: [String: Any]) async throws -> Any {
        let (name, out) = try Self.threadToolCall(method, args)
        do {
            return try await tool(name, out)
        } catch let f as Failure {
            throw f
        } catch {
            let words = LiveSpacesBackend.words(error)
            if Self.missingThreadTool(words) { throw Failure.unsupported("This cua daemon has no Space thread; update Cua") }
            throw Failure(code: "failed", message: words)
        }
    }

    /// The tool and its arguments for one of the thread's methods.
    static func threadToolCall(_ method: String, _ args: [String: Any]) throws -> (String, [String: Any]) {
        guard let space = args["spaceId"] as? String, !space.isEmpty else { throw Failure.badArgs("spaceId: string") }
        var out: [String: Any] = ["space": space]
        switch method {
        case "thread.read":
            if let after = args["after"] as? NSNumber { out["after"] = after }
            if let before = args["before"] as? NSNumber { out["before"] = before }
            if let limit = args["limit"] as? NSNumber { out["limit"] = limit }
            if args["markRead"] as? Bool == true { out["mark_read"] = true }
            if let follow = (args["followMs"] as? NSNumber)?.intValue, follow > 0 {
                out["follow_ms"] = min(follow, 25_000)
                // The stage key last seen: the wait also ends when an agent's stage changes.
                if let live = args["liveAfter"] as? String { out["live_after"] = live }
            }
            // The Agents tab's DM with one agent member (CUA-2266).
            if let with = args["with"] as? String, !with.isEmpty { out["with"] = with }
            return ("thread_read", out)
        case "thread.post":
            guard let text = args["text"] as? String else { throw Failure.badArgs("text: string") }
            out["text"] = text
            if let reply = args["replyTo"] as? NSNumber { out["reply_to"] = reply }
            if let answer = args["answer"] as? [String: Any], let approval = answer["approval"] as? NSNumber {
                var a: [String: Any] = ["approval": approval, "approve": answer["approve"] as? Bool ?? false]
                if let note = answer["note"] as? String, !note.isEmpty { a["note"] = note }
                out["answer"] = a
            }
            if let session = args["toSession"] as? NSNumber { out["to_session"] = session }
            return ("thread_post", out)
        case "thread.members":
            // The Agents tab asks for each agent's DM summary too (CUA-2266).
            if args["overview"] as? Bool == true { out["overview"] = true }
            return ("agent_members", out)
        case "thread.setRights":
            // Member names are unique per machine: the tool takes the name, not the Space.
            guard let name = args["name"] as? String, !name.isEmpty else { throw Failure.badArgs("name: string") }
            var rights: [String: Any] = ["name": name]
            for key in ["task", "control", "approve"] {
                if let group = args[key] as? String {
                    rights[key] = group
                } else if let accounts = args[key] as? [String] {
                    rights[key] = accounts
                }
            }
            return ("agent_set_rights", rights)
        case "thread.agentAdd":
            guard let name = args["name"] as? String, !name.isEmpty else { throw Failure.badArgs("name: string") }
            guard let harness = args["harness"] as? String, !harness.isEmpty else { throw Failure.badArgs("harness: string") }
            out["name"] = name
            out["agent"] = harness
            return ("agent_add", out)
        case "thread.agentRemove":
            // Member names are unique per machine: the tool takes the name.
            guard let name = args["name"] as? String, !name.isEmpty else { throw Failure.badArgs("name: string") }
            return ("agent_remove", ["name": name])
        case "thread.joinCodeCreate":
            if let ttl = args["ttlS"] as? NSNumber { out["ttl_s"] = ttl }
            if let canTask = args["canTask"] as? String {
                guard canTask == "owner" || canTask == "members" else { throw Failure.badArgs("canTask: owner or members") }
                out["can_task"] = canTask
            }
            if let ask = args["askBeforeJoin"] as? Bool { out["ask_before_join"] = ask }
            return ("thread_join_code_create", out)
        case "thread.joinCodeList":
            return ("thread_join_code_list", out)
        case "thread.joinCodeCancel":
            guard let id = args["id"] as? String, !id.isEmpty else { throw Failure.badArgs("id: string") }
            out["id"] = id
            return ("thread_join_code_cancel", out)
        case "thread.settings":
            if let ask = args["askBeforeJoin"] as? Bool { out["ask_before_join"] = ask }
            return ("thread_settings", out)
        default:
            throw Failure.unimplemented(method)
        }
    }

    /// The SDK's `NotFound("Spaces tool thread_read")` or the daemon's
    /// `unknown tool thread_read` (`isMissingThreadTool` in the web bridge).
    static func missingThreadTool(_ message: String) -> Bool {
        let m = message.lowercased()
        let tools = ["thread_read", "thread_post", "agent_members", "thread_join_code_create", "thread_join_code_list",
                     "thread_join_code_cancel", "thread_settings"]
        return tools.contains { t in m.contains("spaces tool \(t)") || ((m.contains("unknown tool") || m.contains("no such tool")) && m.contains(t)) }
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
        // "Add a computer": the relay keeps the one-time code.
        case "devices.createPairingCode":
            let team = (args["team"] as? String).flatMap { $0.isEmpty ? nil : $0 }
            return BridgeValue.encode(try await relay.createPairingCode(team: team))
        case "devices.pairingCodes":
            return BridgeValue.encode(try await relay.pairingCodes())
        case "devices.revokePairingCode":
            try await relay.revokePairingCode(id: try string(args, "id"))
            return NSNull()
        default:
            throw Failure.unimplemented(method)
        }
        await devices.refresh()
        return NSNull()
    }

    /// The page's first run moved past Welcome (its usage notice was on
    /// screen): any page left or shown other than Welcome being shown, or
    /// "Set up later".
    static func leftWelcome(_ raw: Any) -> Bool {
        guard let s = raw as? [String: Any] else { return false }
        switch s["type"] as? String {
        case "onboarding-page":
            // Welcome being shown (`shown`, or the funnel's `step_viewed`) is before the notice.
            let action = s["action"] as? String
            return !(s["page"] as? String == "welcome" && (action == "shown" || action == "step_viewed"))
        case "step": return s["step"] as? String == "onboarding_skipped"
        default: return false
        }
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
        func variant(_ k: String) -> String? {
            guard let v = s[k] as? String, v.count <= 64,
                  v.allSatisfy({ $0.isASCII && ($0.isLetter || $0.isNumber || $0 == "_") }) else { return nil }
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
            // A retried create's final outcome (pages before it send neither).
            let attempt = (s["retryAttempt"] as? NSNumber).map { CFGetTypeID($0) == CFBooleanGetTypeID() ? 2 : $0.intValue } ?? 0
            guard attempt == 0 || attempt == 1,
                  let subclass = s["errorSubclass"] == nil ? "none" : word("errorSubclass") else { return nil }
            return .spaceCreate(location: location, guestOs: os, kind: kind, outcome: outcome, failedPhase: phase,
                                stalled: stalled, elapsedMs: elapsed, gpu: gpu, errorVariant: errorVariant,
                                retryAttempt: UInt32(attempt), errorSubclass: subclass)
        case "space-create-started":
            guard let location = word("location"), let os = word("guestOs"), let kind = word("kind"),
                  let gpu = flag("gpu") else { return nil }
            return .spaceCreateStarted(location: location, guestOs: os, kind: kind, gpu: gpu)
        case "volume-setup":
            guard let surface = word("surface"), let storage = word("storage"), let finder = flag("addToFinder"),
                  let method = word("mountMethod"), let outcome = word("outcome") else { return nil }
            return .volumeSetup(surface: surface, storage: storage, addToFinder: finder, mountMethod: method, outcome: outcome)
        case "share":
            // Why and where a share failed (CUA-1855); a page from before them sends neither.
            guard let action = word("action"), let role = word("role"), let outcome = word("outcome"),
                  let errorKind = s["errorKind"] == nil ? "none" : word("errorKind"),
                  let failedStage = s["failedStage"] == nil ? "none" : word("failedStage") else { return nil }
            return .share(action: action, role: role, outcome: outcome, errorKind: errorKind, failedStage: failedStage)
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
        // A plan limit shown, or its (or an upgrade prompt's) link opened (CUA-1881).
        case "plan-limit-shown":
            guard let surface = word("surface"), let kind = word("kind"), let plan = word("plan") else { return nil }
            return .planLimitShown(surface: surface, kind: kind, plan: plan)
        case "upgrade-link-opened":
            guard let surface = word("surface"), let kind = word("kind"), let plan = word("plan") else { return nil }
            return .upgradeLinkOpened(surface: surface, kind: kind, plan: plan)
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
        watchPanels(of: id, pips: s.pips, rows: s.rows)
        return s
    }

    /// Pops a source out. The desktop shows this window's own viewer
    /// session (the inline desktop's), held while the panel is open: no
    /// second stream opens, and closing the panel hands the same live
    /// session back to the page, with no reconnect. A window (or a window
    /// with no native video) gets a session of its own.
    func popOut(_ source: StreamSource, of id: String,
                stream: (rows: StreamRowsModel, pips: StreamPiPSet)) async throws {
        guard source == .desktop, !stream.pips.isOpen(.desktop), let surfaces = host?.videoSurfaces else {
            stream.pips.popOut(source)
            return
        }
        let session = try await surfaces.holdViewer(spaceId: id)
        heldDesktops.insert(id)
        stream.pips.popOut(.desktop, sharing: session)
    }

    /// Follows a Space's open panels: tells the page when they change (a
    /// panel closed by its own button), and lets go of the viewer session a
    /// closed desktop panel held.
    func watchPanels(of id: String, pips: StreamPiPSet, rows: StreamRowsModel) {
        pipWatches[id] = pips.$openKeys.dropFirst().removeDuplicates().sink { [weak self, weak rows] keys in
            guard let self else { return }
            if !keys.contains(StreamSource.desktop.pipKey), self.heldDesktops.remove(id) != nil {
                self.host?.videoSurfaces?.releaseViewer(spaceId: id)
            }
            let open = rows?.openRows(openKeys: keys) ?? []
            self.host?.emit(Self.pipChangedEvent, payload: ["spaceId": id, "open": open])
        }
    }

    /// The page's `PIP_CHANGED_EVENT` (apps/cua-spaces-web/src/bridge/space-detail.ts).
    static let pipChangedEvent = "stream.pipChanged"

    /// Closes and drops the streams of Spaces that are gone (a loaded
    /// roster only: an empty one is a launch, not a deletion).
    func pruneStreams() {
        guard model.loaded else { return }
        let ids = Set(model.spaces.map(\.id))
        for id in streams.keys where !ids.contains(id) {
            streams[id]?.pips.popInAll()
            streams[id] = nil
            pipWatches[id] = nil
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
            includePasswords: c["includePasswords"] as? Bool ?? false,
            passwordDomains: c["passwordDomains"] as? [String]))
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
