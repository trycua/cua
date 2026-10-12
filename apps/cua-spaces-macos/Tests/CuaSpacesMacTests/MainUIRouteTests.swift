// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesMacKit
import AppKit
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
}
