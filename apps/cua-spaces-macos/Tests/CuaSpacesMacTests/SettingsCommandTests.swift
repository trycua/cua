// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesMacKit
import AppKit
import Foundation
import Testing

/// ⌘, (the app menu's Settings command) opens the New UI's Settings when its
/// window is the one in front, whether or not its web view has focus (it once
/// opened the native SwiftUI Settings window while another dialog had
/// focus); the classic UI keeps the native scene.
@MainActor
@Suite("Settings command")
struct SettingsCommandTests {
    init() { _ = NSApplication.shared }

    /// A window of the app as the running app has them (a process with no
    /// Dock presence, like this test's, reports none as able to be main).
    final class AppWindow: NSWindow {
        override var canBecomeMain: Bool { true }
    }

    /// A panel like the notch: it can be key but never the main window.
    final class UtilityPanel: NSWindow {
        override var canBecomeMain: Bool { false }
    }

    func window() -> NSWindow {
        AppWindow(contentRect: NSRect(x: 0, y: 0, width: 200, height: 120), styleMask: [.titled],
                  backing: .buffered, defer: true)
    }

    typealias Controller = WebUIWindowController

    @Test func theKeyWindowDecidesAndTheFrontmostOneWhenThereIsNone() {
        // The New UI window is key: its Settings, even if another window is listed in front.
        #expect(Controller.settingsRoute(key: .webUI, frontmost: .other) == .web)
        #expect(Controller.settingsRoute(key: .webUI, frontmost: .none) == .web)
        // Another of the app's windows is key (the classic UI, the native Settings): the native scene.
        #expect(Controller.settingsRoute(key: .other, frontmost: .webUI) == .native)
        #expect(Controller.settingsRoute(key: .other, frontmost: .other) == .native)
        // No key window (the app is in the background, a dialog of another app has focus): the front window decides.
        #expect(Controller.settingsRoute(key: .none, frontmost: .webUI) == .web)
        #expect(Controller.settingsRoute(key: .none, frontmost: .other) == .native)
        #expect(Controller.settingsRoute(key: .none, frontmost: .none) == .native)
    }

    @Test func overRealWindows() {
        let web = window(), classic = window()
        // No New UI window at all: the native scene.
        #expect(Controller.settingsRoute(webUI: nil, key: classic, frontmost: classic) == .native)
        #expect(Controller.settingsRoute(webUI: nil, key: nil, frontmost: nil) == .native)
        // The web window is key, with or without focus in its web view.
        #expect(Controller.settingsRoute(webUI: web, key: web, frontmost: web) == .web)
        #expect(Controller.settingsRoute(webUI: web, key: web, frontmost: classic) == .web)
        // The classic main window or the native Settings is key.
        #expect(Controller.settingsRoute(webUI: web, key: classic, frontmost: classic) == .native)
        #expect(Controller.settingsRoute(webUI: web, key: classic, frontmost: web) == .native)
        // Nothing is key; the web window is the one in front (the case of the TCC prompt over the app).
        #expect(Controller.settingsRoute(webUI: web, key: nil, frontmost: web) == .web)
        #expect(Controller.settingsRoute(webUI: web, key: nil, frontmost: classic) == .native)
        #expect(Controller.settingsRoute(webUI: web, key: nil, frontmost: nil) == .native)
    }

    @Test func aUtilityPanelDoesNotCount() {
        let web = window()
        let notch = UtilityPanel(contentRect: NSRect(x: 0, y: 0, width: 80, height: 30), styleMask: [.borderless],
                                 backing: .buffered, defer: true)
        #expect(Controller.settingsFront(notch, webUI: web) == .none)
        #expect(Controller.settingsFront(web, webUI: web) == .webUI)
        #expect(Controller.settingsFront(window(), webUI: web) == .other)
        #expect(Controller.settingsFront(nil, webUI: web) == .none)
        // The notch is key but the New UI window is in front: its Settings.
        #expect(Controller.settingsRoute(webUI: web, key: notch, frontmost: web) == .web)
        #expect(Controller.settingsRoute(webUI: web, key: notch, frontmost: window()) == .native)
    }

    @Test func theFrontmostWindowIsTheFirstVisibleOneThatCanBeMain() {
        let web = window(), classic = window(), settings = window()
        let notch = UtilityPanel(contentRect: NSRect(x: 0, y: 0, width: 80, height: 30), styleMask: [.borderless],
                                 backing: .buffered, defer: true)
        let visible: (NSWindow) -> Bool = { _ in true }
        #expect(Controller.frontmostWindow(in: [notch, web, classic], webUI: web, visible: visible) === web)
        #expect(Controller.frontmostWindow(in: [notch, classic, web], webUI: web, visible: visible) === classic)
        // A hidden window is behind the rest.
        #expect(Controller.frontmostWindow(in: [web, settings], webUI: web, visible: { $0 !== web }) === settings)
        #expect(Controller.frontmostWindow(in: [notch], webUI: web, visible: visible) == nil)
        // The New UI window counts even where a window reports it can't be main.
        let odd = UtilityPanel(contentRect: .zero, styleMask: [.titled], backing: .buffered, defer: true)
        #expect(Controller.frontmostWindow(in: [odd], webUI: odd, visible: visible) === odd)
        #expect(Controller.settingsFront(odd, webUI: odd) == .webUI)
        #expect(Controller.settingsRoute(webUI: odd, key: odd, frontmost: nil) == .web)
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

/// The app menu's one "Settings…" is the `Settings` scene's item; the New UI
/// window answers its action.
@MainActor
@Suite("Settings menu action")
struct SettingsMenuActionTests {
    @Test func theNewUIWindowAnswersTheScenesSettingsAction() {
        _ = NSApplication.shared
        let window = WebUIAppWindow(contentRect: .zero, styleMask: [.titled], backing: .buffered, defer: true)
        #expect(window.responds(to: WebUIAppWindow.settingsAction))
        #expect(!NSWindow(contentRect: .zero, styleMask: [.titled], backing: .buffered, defer: true)
            .responds(to: WebUIAppWindow.settingsAction))
    }
}
