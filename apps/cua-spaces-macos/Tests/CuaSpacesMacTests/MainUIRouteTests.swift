// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSpacesFFI
@testable import CuaSpacesMacKit
import Foundation
import Testing

/// With New UI on, the notch and the menu bar item open the New UI window at
/// the page Electron opens for the same action; with it off, the native
/// window or Settings as before.
@MainActor
@Suite("Main UI routes", .serialized)
struct MainUIRouteTests {
    init() { _ = NSApplication.shared }

    func model(webUi: Bool) -> AppModel {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("cua-route-\(UUID().uuidString)")
        let m = AppModel(backend: FixtureSpacesBackend(), keyvault: KeyvaultModel(client: nil),
                         onboarding: OnboardingModel(statePath: dir.appendingPathComponent("onboarding.json").path),
                         settingsPath: dir.appendingPathComponent("settings.json").path,
                         telemetry: FixtureTelemetry())
        m.settings.experiments = AppExperiments(cuaVolume: true, yourCloud: false, sharing: false, webUi: webUi)
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

    @Test func newUIOnOpensTheWebWindowElseTheNativeOne() {
        for webUi in [true, false] {
            var web: [MainUIRoute] = []
            var native = 0
            let router = MainUIRouter(model: model(webUi: webUi), web: { web.append($0) })
            router.open(.space("s1")) { native += 1 }
            router.open(.settings) { native += 1 }
            #expect(web == (webUi ? [.space("s1"), .settings] : []))
            #expect(native == (webUi ? 0 : 2))
        }
    }

    /// The page gets `route.openRequested {path}`: queued until it listens
    /// (the latest wins), at once after.
    @Test func thePageIsToldWhereToGo() async {
        let c = WebUIWindowController(model: model(webUi: true))
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
        let m = model(webUi: true)
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

    // MARK: - One main window: New UI replaces the native one

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
        let c = WebUIWindowController(model: model(webUi: true), route: MainUIRoute.settings.webPath)
        defer { c.window?.close() }
        #expect(c.startURL.absoluteString.hasSuffix("/settings"))
    }

    @Test func theViewMenusPagesHaveTheirRoutes() {
        #expect(MainUIRouter.route(forNav: "nav.spaces") == .home)
        #expect(MainUIRouter.route(forNav: "nav.machines")?.webPath == "/machines")
        #expect(MainUIRouter.route(forNav: "nav.agents")?.webPath == "/agents")
        #expect(MainUIRouter.route(forNav: "nav.keyvault")?.webPath == "/keyvault")
        #expect(MainUIRouter.route(forNav: "nav.settings") == .settings)
        #expect(MainUIRouter.route(forNav: "nav.nope") == nil)
    }

    /// Only the main UI gives way: the launch and the first run stay in
    /// the native window.
    @Test func onlyTheMainUIGivesWayToNewUI() {
        #expect(MainWindowSwitch.replacesNative(webUi: true, ready: true, onboarded: true))
        #expect(!MainWindowSwitch.replacesNative(webUi: false, ready: true, onboarded: true))
        #expect(!MainWindowSwitch.replacesNative(webUi: true, ready: false, onboarded: true))
        #expect(!MainWindowSwitch.replacesNative(webUi: true, ready: true, onboarded: false))
    }

    func nativeWindow(_ frame: NSRect) -> NSWindow {
        let w = NSWindow(contentRect: frame, styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: true)
        w.isReleasedWhenClosed = false
        w.setFrame(frame, display: false)
        return w
    }

    func settle() async {
        await withCheckedContinuation { k in DispatchQueue.main.async { k.resume() } }
        await withCheckedContinuation { k in DispatchQueue.main.async { k.resume() } }
    }

    /// Whatever opens the native window while New UI is on (the Dock, the
    /// Window menu, a path not routed) gets New UI in its frame; it closes.
    @Test func theNativeWindowOpeningWithNewUIOnIsReplacedInPlace() async {
        let m = model(webUi: true)
        m.onboarding.finish()
        let frame = NSRect(x: 210, y: 160, width: 980, height: 660)
        let w = nativeWindow(frame)
        w.orderFront(nil)
        var opened: [NSRect?] = []
        MainWindowSwitch.nativeAppeared(w, model: m, web: { opened.append($0) })
        // Reported twice (its reader, its onAppear): replaced once.
        MainWindowSwitch.nativeAppeared(nil, model: m, web: { opened.append($0) })
        #expect(opened == [frame])
        await settle()
        #expect(!w.isVisible)
        #expect(w.alphaValue == 1)
    }

    @Test func theNativeWindowStaysWithNewUIOff() async {
        let m = model(webUi: false)
        m.onboarding.finish()
        let w = nativeWindow(NSRect(x: 100, y: 100, width: 900, height: 600))
        w.orderFront(nil)
        defer { w.close() }
        var opened = 0
        MainWindowSwitch.nativeAppeared(w, model: m, web: { _ in opened += 1 })
        await settle()
        #expect(opened == 0 && w.isVisible)
    }

    /// Turning New UI on with the native window up: New UI in its frame.
    @Test func turningNewUIOnSwapsTheWindowInPlace() async {
        let m = model(webUi: true)
        m.onboarding.finish()
        let frame = NSRect(x: 300, y: 200, width: 1000, height: 700)
        let w = nativeWindow(frame)
        w.orderFront(nil)
        MainWindowSwitch.nativeWindow = w
        var opened: [NSRect?] = []
        MainWindowSwitch.switchToWebUI(model: m, web: { opened.append($0) })
        #expect(opened == [frame])
        await settle()
        #expect(!w.isVisible)
        // With no native window up (only Settings), New UI opens as at launch.
        MainWindowSwitch.switchToWebUI(model: m, web: { opened.append($0) })
        #expect(opened == [frame, nil])
    }

    /// Turning it off: the native window opens where New UI was.
    @Test func turningNewUIOffOpensTheNativeWindowInItsFrame() async {
        let m = model(webUi: false)
        m.onboarding.finish()
        let webFrame = NSRect(x: 120, y: 90, width: 1180, height: 760)
        var opened = 0
        MainWindowSwitch.switchToNative(webFrame: webFrame, openMain: { opened += 1 })
        #expect(opened == 1)
        let w = nativeWindow(NSRect(x: 0, y: 0, width: 980, height: 660))
        w.orderFront(nil)
        defer { w.close() }
        MainWindowSwitch.nativeAppeared(w, model: m)
        await settle()
        #expect(w.frame == webFrame)
        #expect(MainWindowSwitch.pendingNativeFrame == nil)
    }

    /// The setting's switch: on puts New UI in the native window's place,
    /// off the reverse.
    @Test func theSwitchSwapsTheWindows() {
        let m = model(webUi: false)
        var calls: [String] = []
        m.openWebUI = { calls.append("open") }
        m.closeWebUI = { calls.append("close") }
        m.chooseExperiment(row: "experiment:web_ui", option: "on")
        #expect(m.settings.experiments.webUi)
        m.chooseExperiment(row: "experiment:web_ui", option: "on")
        m.chooseExperiment(row: "experiment:web_ui", option: "off")
        #expect(calls == ["open", "close"])
    }
}
