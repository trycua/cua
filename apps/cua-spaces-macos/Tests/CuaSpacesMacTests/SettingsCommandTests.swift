// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesMacKit
import AppKit
import Foundation
import Testing

/// Settings is one page, the New UI's: ⌘, (the app menu's "Settings…"), the
/// menu bar item, the notch and the palette all open it, with New UI on or
/// off. There is no native Settings window to drift from it.
@MainActor
@Suite("Settings command")
struct SettingsCommandTests {
    init() { _ = NSApplication.shared }

    func model() -> AppModel {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("cua-settings-\(UUID().uuidString)")
        return AppModel(backend: FixtureSpacesBackend(), keyvault: KeyvaultModel(client: nil),
                        onboarding: OnboardingModel(statePath: dir.appendingPathComponent("onboarding.json").path),
                        settingsPath: dir.appendingPathComponent("settings.json").path,
                        telemetry: FixtureTelemetry())
    }

    /// The classic UI too: Settings never falls back to a native window.
    @Test func settingsIsThePagesWithNewUIOnOrOff() {
        for webUi in [true, false] {
            let m = model()
            m.settings.experiments.webUi = webUi
            var web: [MainUIRoute] = [], settings: [MainUIRoute] = []
            let router = MainUIRouter(model: m, web: { web.append($0) }, settings: { settings.append($0) })
            router.openSettings()
            router.openSettings(.path("/settings/connectors"))
            #expect(settings == [.settings, .path("/settings/connectors")], "webUi \(webUi)")
            #expect(web.isEmpty)
        }
    }

    /// The notch's Settings button goes through the app's hook (the router).
    @Test func theNotchSettingsButtonOpensThePage() {
        let c = NotchController(model: NotchModel(scheduler: ManualScheduler()))
        var calls: [MainUIRoute] = []
        let router = MainUIRouter(model: model(), web: { _ in }, settings: { calls.append($0) })
        c.onOpenSettings = { router.openSettings() }
        c.run(.settings)
        #expect(calls == [.settings])
    }

    /// The page's Settings routes: `/settings` is Account, then one per tab.
    @Test func theSettingsRoutes() {
        #expect(MainUIRoute.settings.webPath == "/settings")
        #expect(MainUIRouter.route(forNav: "nav.settings") == .settings)
    }

    func bridge() -> (WebUIWindowController, WebUIBridge) {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("cua-webui-\(UUID().uuidString)")
        let model = AppModel(backend: FixtureSpacesBackend(), keyvault: KeyvaultModel(client: nil),
                             onboarding: OnboardingModel(statePath: dir.appendingPathComponent("onboarding.json").path),
                             settingsPath: dir.appendingPathComponent("settings.json").path,
                             telemetry: FixtureTelemetry())
        let c = WebUIWindowController(model: model)
        return (c, c.bridge)
    }

    func mainQueueTurn() async {
        await withCheckedContinuation { k in DispatchQueue.main.async { k.resume() } }
    }

    /// The page gets one `settings.openRequested`, at once when it listens and
    /// when it first does otherwise.
    @Test func thePageIsToldToOpenItsSettings() async {
        let (c, bridge) = bridge()
        defer { c.window?.close() }
        var seen: [String] = []
        c.onEmit = { event, _ in seen.append(event) }

        // Before the page has called in: queued, once.
        bridge.requestSettings()
        bridge.requestSettings()
        #expect(seen.isEmpty && bridge.pendingSettings)
        bridge.pageIsListening()
        await mainQueueTurn()
        #expect(seen == ["settings.openRequested"])
        #expect(!bridge.pendingSettings)
        // The page listens: out at once.
        bridge.requestSettings()
        #expect(seen == ["settings.openRequested", "settings.openRequested"])
        // A New Space asked for meanwhile is not lost or mixed up with it.
        bridge.pageListening = false
        bridge.requestNewSpace(on: "host:m1")
        bridge.requestSettings()
        bridge.pageIsListening()
        await mainQueueTurn()
        #expect(seen.suffix(2).sorted() == ["settings.openRequested", "spaces.newRequested"])
    }
}
