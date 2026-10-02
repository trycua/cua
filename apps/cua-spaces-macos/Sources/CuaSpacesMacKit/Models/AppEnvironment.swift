// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
import Foundation

/// Process-level wiring: which backend, where state lives, and the start
/// views captures and UI tests use.
@MainActor
public enum AppEnvironment {
    /// The bundle identity (the Keyvault's trusted-caller requirement names
    /// it; see README "Keyvault identity").
    public static let bundleIdentifier = "com.trycua.spaces.macos"

    /// `$HOME`, so a temp HOME (tests, captures) keeps every file away from
    /// the account's real Library (`FileManager` ignores `$HOME`).
    static var home: URL {
        URL(fileURLWithPath: ProcessInfo.processInfo.environment["HOME"] ?? NSHomeDirectory(), isDirectory: true)
    }

    static var supportDirectory: URL {
        home.appendingPathComponent("Library/Application Support", isDirectory: true)
            .appendingPathComponent(bundleIdentifier, isDirectory: true)
    }

    /// The live model, or fixtures with `CUA_SPACES_FIXTURES=1` (UI tests).
    public static func makeModel() -> AppModel {
        let env = DevHooks.environment
        let fixtures = env["CUA_SPACES_FIXTURES"] == "1"
        let settingsPath = supportDirectory.appendingPathComponent("settings.json").path
        let onboardingPath = supportDirectory.appendingPathComponent("onboarding.json").path
        // The first launch after an update (the version the settings file
        // recorded changed): refresh the agents' skills below. This app's
        // own daemon needs no restart of its own: `cua daemon start` below
        // replaces a daemon whose executable was updated since it started.
        // Fixtures skip it (`CUA_SPACES_REFRESH=1` runs it anyway, for
        // end-to-end runs with a throwaway HOME).
        let info = AboutBundleInfo(bundle: .main)
        // Before the first `Cua`: this process's usage events are the app's
        // (`spaces_app`), wait for the notice the first run shows (never a
        // terminal notice), and start with `app_launched`.
        if !fixtures { _ = appTelemetryStart(version: info.version) }
        var refresh: UpdateRefresh?
        if !fixtures || env["CUA_SPACES_REFRESH"] == "1",
           UpdateRefresh.record(settingsPath: settingsPath, version: info.version, build: info.build,
                                onboarded: OnboardingModel.readCompleted(onboardingPath) ?? false),
           let cua = bundledCua {
            refresh = UpdateRefresh(cua: cua, bundle: Bundle.main.bundlePath)
        }
        let backend: SpacesBackend
        var startError: String?
        let executable = Bundle.main.executablePath ?? CommandLine.arguments[0]
        // This app's own daemon, started before the SDK connects (it
        // replaces another build's, or its own from before an update);
        // fixtures never touch the host.
        let supervisor = fixtures ? nil : bundledCua.flatMap { DaemonSupervisor(bundledCua: $0) }
        var daemonError: String?
        if fixtures {
            backend = FixtureSpacesBackend()
        } else {
            do {
                backend = try makeLiveBackend(supervisor: supervisor, daemonError: &daemonError)
            } catch {
                // Say so: an empty list (and an empty notch) would look
                // like "no Spaces".
                startError = "Could not start the cua SDK: \(LiveSpacesBackend.words(error))"
                NSLog("Cua Spaces: %@", startError!)
                backend = FixtureSpacesBackend(rows: [])
            }
        }
        let keyvault = KeyvaultModel(client: fixtures ? nil : KeyvaultClient(cuaHome: nil))
        if !fixtures {
            // Icons: caches survive restarts; fixtures never resolve apps or
            // touch the network.
            let caches = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask).first?
                .appendingPathComponent("com.trycua.cua-spaces/icons")
            keyvault.appIcons = AppIconCache(dir: caches?.appendingPathComponent("apps"))
            keyvault.siteIconStore = SiteIconStore(dir: caches?.appendingPathComponent("sites"))
            keyvault.iconScale = Int(NSScreen.main?.backingScaleFactor ?? 2)
        }
        let live = backend as? LiveSpacesBackend
        // Fixtures never touch the host service, the account, the telemetry
        // config or the coding agents' configs.
        let host: HostRunning = fixtures || live == nil ? FixtureHost() : CuaSDK.Host(cuaHome: nil)
        let account: AccountRunning? = fixtures ? FixtureAccount() : live.map { LiveAccount(auth: $0.cua.auth()) }
        let telemetry: TelemetryRunning = fixtures ? FixtureTelemetry() : LiveTelemetry()
        let agents: AgentSetupRunning? = fixtures ? FixtureAgentSetup()
            : live.map { LiveAgentSetup(inner: $0.cua.agentSetup(),
                                        cuaBinary: bundledCua) }
        // First run's writes go through the shared core (the Tauri app runs
        // the same installer and host setup); fixtures run none of them.
        let onboarding = OnboardingModel(
            statePath: onboardingPath,
            identity: account?.identity(),
            cli: fixtures ? nil : AppCliInstaller.forExecutable(executable: executable),
            host: host, agentSetup: agents)
        if !fixtures {
            onboarding.onWelcomeLeft = { on in
                do { try appTelemetryWelcomeLeft(on: on) } catch {
                    NSLog("Cua Spaces: the usage-data setting was not saved: %@", LiveSpacesBackend.words(error))
                }
            }
            onboarding.driveTools = backend as? AgentsToolRunning
        }
        // This Mac on the relay: the session's device key (fixtures: an
        // in-memory account; presence answers yes there).
        let devices: DevicesRunning? = fixtures ? FixtureDevices()
            : live.flatMap { LiveDevices.make(auth: $0.cua.auth()) }
        let model = AppModel(backend: backend, keyvault: keyvault, onboarding: onboarding,
                             settingsPath: settingsPath,
                             host: host, account: account, telemetry: telemetry, agentSetup: agents,
                             billing: fixtures ? FixtureBilling() : live.map { LiveBilling(cua: $0.cua) },
                             devices: devices,
                             presence: fixtures ? FixturePresence() : LivePresence(),
                             loginItem: fixtures ? fixtureLoginItem(env["CUA_SPACES_LOGIN_ITEM"]) : MainAppLoginItem())
        if let live {
            let auth = live.cua.auth()
            let token: () async -> String? = { try? await auth.accessToken(force: false) }
            onboarding.accountToken = token
            model.host.accountToken = token
        }
        if let startError { model.show(error: startError) }
        if let live, let supervisor {
            if live.cua.mode() == .daemon {
                daemonSupervision = supervisor.supervise(
                    isUp: { [cua = live.cua] in
                        let probe = await withTimeout(seconds: 5) { try await cua.info() }
                        guard let pid = (try? probe.get())?.daemonPid else { return false }
                        return supervisor.isOwn(pid: pid)
                    },
                    report: { [weak model] error in
                        guard let model else { return }
                        if let error { model.show(error: error) } else if model.bannerIsError { model.banner = nil }
                    })
            } else {
                // Last resort: Spaces run in the app; say what is missing.
                model.show(error: "Cua Spaces could not start its daemon (\(daemonError ?? "it does not answer")). "
                           + "Spaces run inside the app for now; the Keyvault, agents and Cua Volume need the daemon. "
                           + "Reopen Cua Spaces to try again.")
            }
        }
        // Settings → About: Sparkle in the shipped app (a debug build's
        // `CUA_SPACES_UPDATER=live` starts it in a fixtures run too), an
        // in-memory updater for fixtures.
        let channel = model.settings.updateChannel
        let updater: UpdaterDriving? = fixtures && env["CUA_SPACES_UPDATER"] != "live"
            ? FixtureUpdater(lastCheck: Date().addingTimeInterval(-2 * 3600))
            : SparkleUpdater.start(channels: appAboutAllowedChannels(channel: channel))
        model.updates = UpdatesModel(updater: updater, channel: channel)
        model.updates.telemetry = telemetry
        model.updates.saveChannel = { [weak model] channel in
            model?.settings.updateChannel = channel
            model?.saveSettings()
        }
        // End-to-end runs: check now, with no window unless one is found.
        if env["CUA_SPACES_UPDATE_CHECK"] == "background", let sparkle = updater as? SparkleUpdater {
            sparkle.checkInBackground()
        }
        if let refresh {
            let daemon = daemonError
            Task.detached {
                let agents = refresh.updateAgents()
                let notice = appAboutRefreshNotice(report: AppRefreshReport(agentsError: agents, daemonError: daemon))
                NSLog("Cua Spaces: refreshed after an update (agents: %@, daemon: %@)",
                      agents ?? "ok", daemon ?? "ok")
                if let notice { await MainActor.run { model.show(info: notice) } }
            }
        }
        return model
    }

    /// Fixtures' login item, never the Mac's: `CUA_SPACES_LOGIN_ITEM` sets
    /// what it reports (`enabled`, `notRegistered`, `requiresApproval`,
    /// `notFound`; default enabled), for captures of each state.
    static func fixtureLoginItem(_ status: String?) -> FixtureLoginItem {
        switch status ?? "" {
        case "notRegistered": return FixtureLoginItem(.notRegistered)
        case "requiresApproval": return FixtureLoginItem(.requiresApproval)
        case "notFound": return FixtureLoginItem(.notFound)
        default: return FixtureLoginItem(.enabled)
        }
    }

    /// The supervision of this app's daemon ([`DaemonSupervisor`]), for the
    /// app's lifetime.
    static var daemonSupervision: Task<Void, Never>?

    /// The live backend on this app's own daemon: `cua daemon start` first
    /// (bounded; `daemonError` says why it failed), then the SDK, which uses
    /// only this build's daemon. When another build's daemon came up in
    /// between, the SDK refuses it with `DaemonNotRunning`, and one more
    /// start replaces it. Without a daemon the SDK runs Spaces in the app.
    static func makeLiveBackend(supervisor: DaemonSupervisor?, daemonError: inout String?) throws -> LiveSpacesBackend {
        daemonError = supervisor?.start()
        do {
            return try LiveSpacesBackend.make()
        } catch CuaError.DaemonNotRunning(let message) where supervisor != nil {
            if let error = supervisor?.start() { throw CuaError.DaemonNotRunning(message: "\(message) (\(error))") }
            return try LiveSpacesBackend.make()
        }
    }

    /// The `cua` this app bundles, when it has one.
    static var bundledCua: String? {
        let executable = Bundle.main.executablePath ?? CommandLine.arguments[0]
        let cua = URL(fileURLWithPath: executable).deletingLastPathComponent().appendingPathComponent("cua").path
        return FileManager.default.isExecutableFile(atPath: cua) ? cua : nil
    }

    /// Opens this app's own menu bar item (captures): clicks its status
    /// bar button.
    static func openMenuBarMenu() {
        for window in NSApp.windows where String(describing: type(of: window)).contains("StatusBar") {
            if let button = find(NSStatusBarButton.self, in: window.contentView) {
                button.performClick(nil)
                return
            }
        }
    }

    private static func find<T: NSView>(_ type: T.Type, in view: NSView?) -> T? {
        guard let view else { return nil }
        if let hit = view as? T { return hit }
        for sub in view.subviews { if let hit = find(type, in: sub) { return hit } }
        return nil
    }

    /// `CUA_SPACES_START_VIEW`: `new-space`, `new-space-picker`, `new-space-resources`, `keyvault`, `keyvault-waiting`,
    /// `approval`, `onboarding`, `onboarding-mode`, `this-machine`, `host-setup`,
    /// `host-configured`, `settings`, `settings-devices` (Settings on its
    /// Devices tab), `settings-about` (on its About tab), `settings-experiments`
    /// (on its Experiments tab), `device-approval` (the approval sheet for the first
    /// device asking; `CUA_SPACES_APPROVE_CODE` types its code),
    /// `device-enroll` (the enroll sheet), `device-enroll-code` (it, having
    /// chosen Approve from another device), `space`, `space-window` (the selected
    /// Space's own window), the notch states `notch-open`
    /// (`notch`; 1.5 s after launch), `notch-hover`, `notch-permission`,
    /// `notch-drag-target` (`notch-prompt`) and `notch-drop`; `notch-menu` (the notch
    /// open and the menu bar menu) and `menu` (the menu alone; `menu-popup`
    /// shows its items as a context menu over the main window); the onboarding
    /// pages `onboarding-signin`, `-agents`, `-presentation`, `-drive`
    /// (`-drive-bucket`: Your S3 bucket's prompt, `-drive-manual`: its
    /// fields), `-mode`,
    /// `-host-form`, `-done`;
    /// `CUA_SPACES_CREATE` (`<image>[,<name>]`) creates a Space on this Mac
    /// through the New Space wizard's plan and the real create path, as if
    /// Create were pressed (end-to-end runs and captures), and
    /// `CUA_SPACES_CANCEL_AFTER` (seconds) presses its Cancel then;
    /// `CUA_SPACES_SELECT` selects a Space; `CUA_SPACES_NOTCH_HIGHLIGHT`
    /// (`tile`, `list`, `settings`, `search` or `tab`, optionally
    /// `:pressed`) forces a notch control's hover or pressed look. Static
    /// start states only.
    static func applyStartView(_ model: AppModel, notch: NotchController) {
        let env = DevHooks.environment
        Task {
            await model.refresh()
            await model.keyvault.refresh()
            if let id = env["CUA_SPACES_SELECT"] { model.select(id) }
            if let spec = env["CUA_SPACES_CREATE"], !spec.isEmpty {
                let parts = spec.split(separator: ",", maxSplits: 1).map(String.init)
                await model.openNewSpace()
                let w = model.wizard
                w.send(.setPlacement(placement: .local))
                w.send(.setImageText(text: parts[0]))
                w.send(.pickImageSuggestion)
                if parts.count > 1 { w.send(.setName(name: parts[1])) }
                model.create(w.view.plan)
                if let after = env["CUA_SPACES_CANCEL_AFTER"].flatMap(Double.init),
                   let id = model.creates.pending.last?.id {
                    Task {
                        try? await Task.sleep(for: .seconds(after))
                        model.cancelCreate(id)
                    }
                }
            }
            switch env["CUA_SPACES_START_VIEW"] ?? "" {
            case "new-space":
                await model.openNewSpace()
            case "new-space-picker":
                await model.openNewSpace()
                model.wizard.send(.openImageSuggestions)
            case "new-space-resources":
                // The Resources step for `CUA_SPACES_WIZARD_IMAGE` on this
                // Mac (or `CUA_SPACES_WIZARD_ON=cloud`), with
                // `CUA_SPACES_WIZARD_DISK` GB when its disk grows and
                // `CUA_SPACES_WIZARD_CPUS` / `CUA_SPACES_WIZARD_MEMORY` (GB).
                // Your cloud needs its experiment (this run only).
                if env["CUA_SPACES_WIZARD_ON"] == "yours" { model.settings.experiments.yourCloud = true }
                await model.openNewSpace()
                let w = model.wizard
                if let image = env["CUA_SPACES_WIZARD_IMAGE"] { w.send(.chooseImage(imageRef: image)) }
                w.send(.setPlacement(placement: env["CUA_SPACES_WIZARD_ON"] == "cloud" ? .cloud
                                   : env["CUA_SPACES_WIZARD_ON"] == "yours" ? .yours : .local))
                w.send(.next)
                if let disk = env["CUA_SPACES_WIZARD_DISK"].flatMap(UInt32.init) { w.send(.setDisk(diskGb: disk)) }
                if let cpus = env["CUA_SPACES_WIZARD_CPUS"].flatMap(UInt32.init) { w.send(.setCpus(cpus: cpus)) }
                if let memory = env["CUA_SPACES_WIZARD_MEMORY"].flatMap(UInt32.init) { w.send(.setMemory(memoryGb: memory)) }
            case "keyvault":
                model.selection = .keyvault(.category(category: .all))
            case "keyvault-waiting":
                model.selection = .keyvault(.category(category: .waiting))
            case "approval":
                model.selection = .keyvault(.category(category: .waiting))
                if let first = model.keyvault.overview.pending.first { model.keyvault.openApproval(first.id) }
            case "onboarding":
                model.onboarding.restart()
            case "onboarding-mode":
                model.onboarding.restart()
                model.onboarding.send(.start)
                model.onboarding.send(.signinDone)
                model.onboarding.send(.agentsDone(configured: []))
                model.onboarding.send(.presentationDone)
                model.onboarding.send(.driveContinue)
            case "onboarding-host-form":
                model.onboarding.restart()
                model.onboarding.send(.start)
                model.onboarding.send(.signinDone)
                model.onboarding.send(.agentsDone(configured: []))
                model.onboarding.send(.presentationDone)
                model.onboarding.send(.driveContinue)
                model.onboarding.host.openForm()
            case "onboarding-signin":
                model.onboarding.restart()
                model.onboarding.send(.start)
            case "onboarding-agents":
                model.onboarding.restart()
                model.onboarding.send(.start)
                model.onboarding.send(.signinDone)
            case "onboarding-presentation":
                model.onboarding.restart()
                model.onboarding.send(.start)
                model.onboarding.send(.signinDone)
                model.onboarding.send(.agentsDone(configured: []))
            case "onboarding-drive", "onboarding-drive-bucket", "onboarding-drive-manual":
                // The Volume page shows with the Cua Volume experiment on
                // (for this run only; the settings file is not written).
                model.settings.experiments.cuaVolume = true
                model.onboarding.restart()
                model.onboarding.send(.start)
                model.onboarding.send(.signinDone)
                model.onboarding.send(.agentsDone(configured: []))
                model.onboarding.send(.presentationDone)
                if env["CUA_SPACES_START_VIEW"] != "onboarding-drive" {
                    // Your S3 bucket: the agent prompt, or its fields.
                    await model.onboarding.checkDrive()
                    model.onboarding.send(.storageChosen(choice: .s3))
                    if env["CUA_SPACES_START_VIEW"] == "onboarding-drive-manual" {
                        model.onboarding.send(.driveStorage(action: .showManual(on: true)))
                    }
                }
            case "drive":
                // The Volume page shows with Cua Volume on (this run only).
                model.settings.experiments.cuaVolume = true
                model.selection = .drive
            case "onboarding-done":
                model.onboarding.restart()
                model.onboarding.send(.start)
                model.onboarding.send(.signinDone)
                model.onboarding.send(.agentsDone(configured: []))
                model.onboarding.send(.presentationDone)
                model.onboarding.send(.driveContinue)
                model.onboarding.send(.modeChosen(mode: .client))
            case "settings", "settings-devices", "settings-storage", "settings-about", "settings-experiments":
                if env["CUA_SPACES_START_VIEW"] == "settings-devices" { model.settingsTab = .devices }
                if env["CUA_SPACES_START_VIEW"] == "settings-about" { model.settingsTab = .about }
                if env["CUA_SPACES_START_VIEW"] == "settings-experiments" { model.settingsTab = .experiments }
                if env["CUA_SPACES_START_VIEW"] == "settings-storage" {
                    // Storage shows with the Cua Volume experiment on (this
                    // run only; the settings file is not written).
                    model.settings.experiments.cuaVolume = true
                    model.settingsSection = "storage"
                }
                try? await Task.sleep(for: .seconds(2))
                NSApp.activate()
                if let item = NSApp.mainMenu?.items.first?.submenu?.items.first(where: { $0.keyEquivalent == "," }),
                   let action = item.action {
                    NSApp.sendAction(action, to: item.target, from: item)
                }
            case "device-approval":
                // The approval sheet for the first device asking (it opens
                // on its own once the relay lists one).
                await model.devices.refresh()
                if model.devices.approval == nil, let first = model.devices.view.approvals.first {
                    model.devices.openApproval(first, in: .main)
                }
                if let code = env["CUA_SPACES_APPROVE_CODE"] { model.devices.setCode(code) }
            case "device-enroll":
                model.devices.startEnroll(in: .main)
            case "device-enroll-code":
                // "Approve from another device": register, show the code, wait.
                model.devices.startEnroll(in: .main)
                await model.devices.chooseEnroll(.approve)
            case "space":
                model.select("local:aurora")
            case "this-machine":
                model.select("this-mac")
            case "host-setup":
                model.select("this-mac")
                model.host.openForm()
            case "host-configured":
                model.select("this-mac")
                model.host.openForm()
                model.host.send(.setName(name: "Studio"))
                await model.host.submit()
            case "notch", "notch-open":
                // Closed first, then open once after 1.5 s, so a capture
                // sees the spring.
                try? await Task.sleep(for: .seconds(1.5))
                model.notch.send(.click)
            case "notch-menu":
                // The notch open, then the menu bar item's menu (both ours).
                try? await Task.sleep(for: .seconds(1.5))
                model.notch.send(.click)
                try? await Task.sleep(for: .seconds(1.5))
                openMenuBarMenu()
            case "menu":
                try? await Task.sleep(for: .seconds(1.5))
                openMenuBarMenu()
            case "menu-popup":
                // The menu bar item's items as a context menu over the main
                // window (captures on a crowded menu bar, where the item
                // can be hidden), once the sync status has been read.
                try? await Task.sleep(for: .seconds(4))
                await model.persistent.refreshSync()
                NSApp.activate()
                guard let view = NSApp.windows.first(where: { $0.isVisible && $0.contentView != nil
                    && $0.frame.width > 400 })?.contentView else { break }
                let menu = NSMenu()
                for item in model.menuBar {
                    if item.id == .separator { menu.addItem(.separator()); continue }
                    let m = NSMenuItem(title: item.label, action: nil, keyEquivalent: "")
                    m.isEnabled = item.enabled
                    menu.addItem(m)
                }
                menu.autoenablesItems = false
                menu.popUp(positioning: nil, at: NSPoint(x: 300, y: view.isFlipped ? 120 : view.bounds.height - 120), in: view)
            case "notch-activity-hotspot":
                model.notch.setActivity(hotspot: true, transfer: nil)
            case "notch-activity-transfer":
                model.notch.setActivity(hotspot: false, transfer: AppNotchTransfer(sent: 600, total: 1000))
            case "notch-activity-provisioning":
                if var s = model.notch.spaces.first(where: { $0.id != "this-mac" }) {
                    s.id = "cloud:starter"
                    s.name = "Starter"
                    s.status = .provisioning
                    s.startedAt = Int64(Date().timeIntervalSince1970 * 1000) - 40_000
                    model.notch.spaces.append(s)
                }
            case "notch-hover":
                model.notch.send(.hoverEnter)
                model.notch.send(.dragPermission(granted: true))
            case "notch-permission":
                model.notch.send(.dragPermission(granted: false))
                model.notch.send(.click)
            case "notch-prompt", "notch-drag-target":
                model.notch.send(.drag(event: .start(windowId: nil, appName: env["CUA_SPACES_DRAG_APP"])))
            case "notch-drop":
                // The drop panel with a tile under the dragged window.
                model.notch.send(.drag(event: .start(windowId: nil, appName: env["CUA_SPACES_DRAG_APP"])))
                model.notch.send(.drag(event: .enterNotch))
                if let first = model.notch.view.tiles.first(where: \.dropTarget) {
                    model.notch.send(.drag(event: .over(spaceId: first.id)))
                }
            default:
                break
            }
            // A forced hover or pressed look on one notch control (static;
            // no input is replayed).
            if let raw = env["CUA_SPACES_NOTCH_HIGHLIGHT"] {
                let first = model.notch.view.tiles.first(where: \.dropTarget)?.id
                model.notch.highlight = NotchHighlight.parse(raw, firstTile: first)
            }
        }
    }
}
