// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSpacesFFI
@testable import CuaSpacesMacKit
import Foundation
import Testing

/// The notch, the menu bar item, the menus and notifications open the web UI
/// window (the only main window) at the page Electron opens for the same
/// action.
@MainActor
@Suite("Main UI routes", .serialized)
struct MainUIRouteTests {
    init() { _ = NSApplication.shared }

    func model() -> AppModel {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("cua-route-\(UUID().uuidString)")
        let m = AppModel(backend: FixtureSpacesBackend(), keyvault: KeyvaultModel(client: nil),
                         onboarding: OnboardingModel(statePath: dir.appendingPathComponent("onboarding.json").path),
                         settingsPath: dir.appendingPathComponent("settings.json").path,
                         telemetry: FixtureTelemetry())
        m.settings.experiments = AppExperiments(cuaVolume: true, ciRunners: false, tcpShares: false, mobile: false)
        return m
    }

    @Test func eachRouteIsThePagesPath() {
        #expect(MainUIRoute.home.webPath == "/spaces")
        #expect(MainUIRoute.settings.webPath == "/settings")
        #expect(MainUIRoute.access.webPath == "/keyvault?view=access")
        #expect(MainUIRoute.drive.webPath == "/volume")
        #expect(MainUIRoute.space("s 1/x").webPath == "/spaces/s%201%2Fx")
        let drop = MainUIRoute.space("s1", dropped: [URL(fileURLWithPath: "/Applications/Slack.app"),
                                                     URL(fileURLWithPath: "/tmp/a b.txt")])
        // encodeURIComponent(JSON.stringify(["Slack.app","a b.txt"])), as Electron's.
        #expect(drop.webPath == "/spaces/s1?dropped=%5B%22Slack.app%22%2C%22a%20b.txt%22%5D")
        #expect(drop.dropped.map(\.lastPathComponent) == ["Slack.app", "a b.txt"])
        #expect(MainUIRoute.home.dropped.isEmpty)
    }

    /// Every route opens the web UI window (there is no native window to
    /// fall back to).
    @Test func everyRouteOpensTheWebWindow() {
        let m = model()
        var web: [MainUIRoute] = []
        let router = MainUIRouter(model: m, web: { web.append($0) })
        router.open(.space("s1"))
        router.open(.settings)
        #expect(web == [.space("s1"), .settings])
    }

    /// Open, the Dock icon, ⌘⇧Space: the first run while it is due, else Spaces.
    @Test func theMainWindowIsTheFirstRunUntilItIsDone() {
        let m = model()
        var web: [MainUIRoute] = []
        let router = MainUIRouter(model: m, web: { web.append($0) })
        router.openMain()
        m.onboarding.finish()
        router.openMain()
        // The model's state file does not exist yet: the app's first launch here.
        #expect(web == [.firstLaunch, .home])
        #expect(MainUIRouter.launchRoute(onboarded: false) == .firstRunDue)
        #expect(MainUIRouter.launchRoute(onboarded: false, firstLaunch: true) == .firstLaunch)
        #expect(MainUIRouter.launchRoute(onboarded: true) == nil)
    }

    /// A click on a notification opens its page.
    @Test func aNotificationOpensItsPage() {
        let ready = SpaceReadyNote.identifierPrefix + "local:aurora"
        #expect(MainUIRouter.route(forNotification: ready) == .space("local:aurora"))
        #expect(MainUIRouter.route(forNotification: "device-abc") == .home)
        #expect(MainUIRouter.route(forNotification: "agent-42") == .agents)
        #expect(MainUIRouter.route(forNotification: "other") == .home)
    }

    /// The page gets `route.openRequested {path}`: queued until it listens
    /// (the latest wins), at once after.
    @Test func thePageIsToldWhereToGo() async {
        let c = WebUIWindowController(model: model())
        defer { c.window?.close() }
        let bridge = c.bridge
        var seen: [String] = []
        c.onEmit = { event, payload in
            seen.append("\(event) \((payload as? [String: Any])?["path"] as? String ?? "")")
        }
        bridge.requestRoute("/settings")
        bridge.requestRoute("/spaces/s1")
        #expect(seen.isEmpty && bridge.pendingRoute == "/spaces/s1")
        bridge.pageIsListening()
        await withCheckedContinuation { k in DispatchQueue.main.async { k.resume() } }
        #expect(seen == ["route.openRequested /spaces/s1"])
        #expect(bridge.pendingRoute == nil)
        bridge.requestRoute("/spaces")
        #expect(seen.last == "route.openRequested /spaces")
    }

    /// A drop on a notch tile reaches the page's well by name, once, and is
    /// then sendable.
    @Test func aNotchDropIsHandedToThePageOnce() async throws {
        let m = model()
        await m.refresh()
        let id = try #require(m.spaces.first?.id)
        let c = WebUIWindowController(model: m)
        defer { c.window?.close(); WebUIWindowController.notchDrop = [] }
        let bridge = c.bridge
        let board = NSPasteboard(name: NSPasteboard.Name("cua-test-drag-\(UUID().uuidString)"))
        defer { board.releaseGlobally() }
        board.clearContents()
        bridge.dragPasteboard = { board }

        WebUIWindowController.notchDrop = MainUIRoute.space(id, dropped: [URL(fileURLWithPath: "/tmp/n.txt")]).dropped
        #expect(try await bridge.handle("spaces.droppedFiles", ["names": ["n.txt"]]) as? [String] == ["/tmp/n.txt"])
        #expect(WebUIWindowController.notchDrop.isEmpty)
        #expect(try await bridge.handle("spaces.droppedFiles", ["names": ["n.txt"]]) as? [String] == [])
        #expect(try await bridge.handle("spaces.sendFiles", ["spaceId": id, "paths": ["/tmp/n.txt"]]) is [Any])
    }

    /// The header's list and Settings buttons go through the app's hooks
    /// when set, else the notch view's own actions.
    @Test func theNotchHeaderButtonsUseTheAppsHooks() {
        let c = NotchController(model: NotchModel(scheduler: ManualScheduler()))
        var calls: [String] = []
        c.attach(openMain: { calls.append("main") }, openSettings: { calls.append("native settings") })
        c.run(.list)
        c.run(.settings)
        c.onOpenList = { calls.append("list") }
        c.onOpenSettings = { calls.append("settings") }
        c.run(.list)
        c.run(.settings)
        #expect(calls == ["main", "native settings", "list", "settings"])
    }

    // MARK: - The web UI window

    /// A new window loads the page at its route (query included) rather
    /// than asking a page that may not listen yet.
    @Test func aNewWindowStartsAtItsRoute() {
        let base = URL(string: "cua-spaces://app/")!
        #expect(WebUIWindowController.url("/settings", in: base).absoluteString == "cua-spaces://app/settings")
        #expect(WebUIWindowController.url(MainUIRoute.access.webPath, in: base).absoluteString
                == "cua-spaces://app/keyvault?view=access")
        #expect(WebUIWindowController.url("spaces/s1", in: base).absoluteString == "cua-spaces://app/spaces/s1")
        let dev = URL(string: "http://localhost:5174/")!
        #expect(WebUIWindowController.url("/spaces/a%20b?dropped=%5B%5D", in: dev).absoluteString
                == "http://localhost:5174/spaces/a%20b?dropped=%5B%5D")
        let c = WebUIWindowController(model: model(), route: MainUIRoute.settings.webPath)
        defer { c.window?.close() }
        #expect(c.startURL.absoluteString.hasSuffix("/settings"))
    }

    @Test func theViewMenusPagesHaveTheirRoutes() {
        #expect(MainUIRouter.route(forNav: "nav.spaces") == .home)
        #expect(MainUIRouter.route(forNav: "nav.machines") == nil)
        #expect(MainUIRouter.route(forNav: "nav.agents")?.webPath == "/agents")
        #expect(MainUIRouter.route(forNav: "nav.keyvault")?.webPath == "/keyvault")
        #expect(MainUIRouter.route(forNav: "nav.settings") == .settings)
        #expect(MainUIRouter.route(forNav: "nav.nope") == nil)
    }

    /// A prerelease's first launch (a nightly, or a beta from before
    /// Nightly) starts on Nightly; a release's on Stable; a later launch
    /// keeps the channel.
    @Test func aPrereleasesFirstLaunchIsOnNightly() {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("cua-newui-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let path = dir.appendingPathComponent("settings.json").path
        UpdateRefresh.record(settingsPath: path, version: "0.8.1-beta.1", build: "1", onboarded: false)
        #expect(appSettingsLoad(path: path).updateChannel == .nightly)
        // A later launch keeps the channel the user set.
        var s = appSettingsLoad(path: path)
        s.updateChannel = .stable
        try? appSettingsSave(path: path, settings: s)
        UpdateRefresh.record(settingsPath: path, version: "0.8.1-beta.2", build: "2", onboarded: true)
        #expect(appSettingsLoad(path: path).updateChannel == .stable)
        let release = FileManager.default.temporaryDirectory.appendingPathComponent("cua-newui-\(UUID().uuidString)/settings.json").path
        UpdateRefresh.record(settingsPath: release, version: "0.7.2", build: "1", onboarded: false)
        #expect(appSettingsLoad(path: release).updateChannel == .stable)
    }
}
