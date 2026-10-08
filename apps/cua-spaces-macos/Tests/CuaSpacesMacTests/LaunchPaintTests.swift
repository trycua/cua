// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesMacKit
import AppKit
import CuaSpacesFFI
import Foundation
import Testing
import WebKit

/// From launch to the New UI window's first frame nothing waits: not on a
/// file read stuck behind a macOS privacy prompt ("would like to access data
/// from other apps"), not on the daemon, not on the live services. A release candidate once
/// showed a blank white window for about 20 s. Here the live services never
/// arrive (their start is a blocking read that takes seconds), and every
/// answer the page needs for its first render must still come at once.
@MainActor
@Suite struct LaunchPaintTests {
    /// What the page asks while it renders its first screen.
    static let firstRender: [(String, [String: Any])] = [
        ("app.info", [:]), ("startup.get", [:]), ("session.get", [:]), ("settings.get", [:]),
        ("spaces.list", [:]), ("spaces.createOptions", [:]),
        ("machines.list", [:]), ("host.status", [:]), ("keyvault.get", [:]),
        ("about.get", [:]), ("loginItem.get", [:]),
    ]

    /// How long one answer may take (generous: the machine may be loaded;
    /// what waits for the launch never answers at all).
    static let budget: Duration = .seconds(5)

    /// A model as `AppEnvironment.makeModel` makes it, on stand-ins for the
    /// live services, whose start is `start` (never resolving the gate).
    func launchingModel(start: @escaping () async -> Void) -> (AppModel, LiveGate<LiveServices>) {
        let gate = LiveGate<LiveServices>()
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("cua-paint-\(UUID().uuidString)")
        let startup = StartupModel(phase: .starting)
        startup.keychain = FixtureKeychain(quiet: .ready)
        startup.start = start
        let model = AppModel(backend: PendingSpacesBackend(gate: gate), keyvault: KeyvaultModel(client: nil),
                             onboarding: OnboardingModel(statePath: dir.appendingPathComponent("onboarding.json").path),
                             settingsPath: dir.appendingPathComponent("settings.json").path,
                             host: PendingHost(gate: gate), account: PendingAccount(gate: gate),
                             telemetry: FixtureTelemetry(), agentSetup: PendingAgentSetup(gate: gate),
                             billing: PendingBilling(gate: gate), devices: PendingDevices(gate: gate),
                             presence: FixturePresence(), loginItem: FixtureLoginItem(.enabled), startup: startup)
        return (model, gate)
    }

    /// The bridge's answer, or nil when none came within `budget` (the
    /// call is left waiting: a hang fails here instead of hanging the run).
    func answer(_ bridge: WebUIBridge, _ method: String, _ args: [String: Any],
                within budget: Duration = LaunchPaintTests.budget) async -> Any? {
        final class Box { var value: Any?; var done = false }
        let box = Box()
        Task { @MainActor in
            box.value = (try? await bridge.handle(method, args)) ?? NSNull()
            box.done = true
        }
        let deadline = ContinuousClock.now + budget
        while !box.done, ContinuousClock.now < deadline { try? await Task.sleep(for: .milliseconds(5)) }
        return box.done ? box.value : nil
    }

    /// A read that blocks its thread (a file behind a privacy prompt).
    nonisolated static func blockingRead(seconds: Double) {
        Thread.sleep(forTimeInterval: seconds)
    }

    /// Runs a blocking read on a thread of its own: off the main thread, and
    /// off the few threads every Swift task shares, which a blocked read
    /// would hold for its whole wait (on a small CI machine that stalls the
    /// tests running alongside, such as the timeout tests).
    nonisolated static func onOwnThread(_ read: @escaping @Sendable () -> Void) async {
        await withCheckedContinuation { (done: CheckedContinuation<Void, Never>) in
            Thread.detachNewThread {
                read()
                done.resume()
            }
        }
    }

    @Test func theFirstRendersAnswersComeAtOnceWhileTheStartIsStuck() async {
        let blocked = LiveGate<Bool>()
        // The start runs the slow read off the main thread, as makeModel's
        // does, and then never hands the services over.
        let (model, gate) = launchingModel {
            await Self.onOwnThread { Self.blockingRead(seconds: 3) }
            _ = await blocked.wait()
        }
        model.startup.begin()
        let bridge = WebUIBridge(model: model, allowedOrigins: [])
        for (method, args) in Self.firstRender {
            #expect(await answer(bridge, method, args) != nil, "\(method) waited for the launch")
        }
        #expect(gate.current == nil)
        // What says "still launching" says so.
        let options = await answer(bridge, "spaces.createOptions", [:]) as? [String: Any]
        #expect(options?["pending"] as? Bool == true)
        let startup = await answer(bridge, "startup.get", [:]) as? [String: Any]
        #expect(startup?["phase"] as? String == "starting")
        #expect(await answer(bridge, "host.status", [:]) is NSNull)
        let spaces = await answer(bridge, "spaces.list", [:]) as? [String: Any]
        #expect(spaces?["loaded"] as? Bool == false)
        blocked.resolve(true)
    }

    /// A read stuck behind a prompt until the test answers it.
    final class StuckRead: @unchecked Sendable {
        let answered = DispatchSemaphore(value: 0)
        private let lock = NSLock()
        private var _reading = false
        var reading: Bool { lock.withLock { _reading } }
        func read() {
            lock.withLock { _reading = true }
            answered.wait()
        }
    }

    @Test func theMainThreadKeepsRunningWhileTheStartIsStuckOnARead() async {
        let stuck = StuckRead()
        let (model, _) = launchingModel {
            await Self.onOwnThread { stuck.read() }
        }
        model.startup.begin()
        let bridge = WebUIBridge(model: model, allowedOrigins: [])
        // The read is waiting, and the main actor still answers the page.
        let deadline = ContinuousClock.now + .seconds(20)
        while !stuck.reading, ContinuousClock.now < deadline { try? await Task.sleep(for: .milliseconds(10)) }
        #expect(stuck.reading)
        let state = await answer(bridge, "startup.get", [:], within: .seconds(10)) as? [String: Any]
        #expect(state?["phase"] as? String == "starting")
        #expect(stuck.reading)
        stuck.answered.signal()
    }

    /// Before the page paints, the window shows the theme's colour, never
    /// the web view's white.
    @Test func theWindowShowsTheThemeUntilThePagePaints() throws {
        let (model, _) = launchingModel {}
        let c = WebUIWindowController(model: model)
        defer { c.window?.close() }
        #expect(c.webView.value(forKey: "drawsBackground") as? Bool == false)
        let window = try #require(c.window)
        let expected = WebUIWindowController.storedBackground(for: window.effectiveAppearance)
        #expect(window.backgroundColor.webHex == expected.webHex)
        // Until the page reports one: the page's own colours, not white.
        #expect(WebUIWindowController.defaultBackground.light == "#f7f8fa")
        #expect(WebUIWindowController.defaultBackground.dark == "#16181c")
        // An older build's kept white is not used.
        let key = WebUIWindowController.backgroundKey(for: window.effectiveAppearance)
        let before = UserDefaults.standard.string(forKey: key)
        defer { UserDefaults.standard.set(before, forKey: key) }
        UserDefaults.standard.set("#ffffff", forKey: key)
        #expect(WebUIWindowController.storedBackground(for: window.effectiveAppearance).webHex != "#ffffff")
    }
}
