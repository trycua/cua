// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSpacesFFI
@testable import CuaSpacesMacKit
import Foundation
import Testing

/// The first run on the Mac is the shared web onboarding: what the bridge
/// answers it (`onboarding.get`, `onboarding.complete`), the Space it
/// creates ("<Space> is ready", `notifyWhenReady`) and the usage notice it
/// shows.
@MainActor
@Suite("First run (web onboarding)", .serialized)
struct FirstRunTests {
    init() { _ = NSApplication.shared }

    func bridge(webUi: Bool = false, backend: SpacesBackend = FixtureSpacesBackend(),
                loginItem: LoginItemControlling? = nil) -> (WebUIBridge, AppModel, FixtureTelemetry, URL) {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("cua-firstrun-\(UUID().uuidString)")
        let state = dir.appendingPathComponent("onboarding.json")
        let telemetry = FixtureTelemetry()
        let model = AppModel(backend: backend, keyvault: KeyvaultModel(client: nil),
                             onboarding: OnboardingModel(statePath: state.path),
                             settingsPath: dir.appendingPathComponent("settings.json").path, telemetry: telemetry,
                             loginItem: loginItem)
        model.settings.experiments = AppExperiments(cuaVolume: false, yourCloud: false, sharing: true, webUi: webUi)
        let bridge = WebUIBridge(model: model, allowedOrigins: [])
        bridge.notifySpaceReady = { _ in }
        return (bridge, model, telemetry, state)
    }

    func yield() async {
        await withCheckedContinuation { k in DispatchQueue.main.async { k.resume() } }
    }

    // MARK: - The first run's window

    /// The first run is the web onboarding, with New UI on or off: once the
    /// launch is ready and until it is done, the native window gives way.
    @Test func theFirstRunIsTheWebOnboardingWhateverTheNewUISetting() {
        #expect(MainWindowSwitch.opensFirstRun(ready: true, onboarded: false))
        #expect(!MainWindowSwitch.opensFirstRun(ready: false, onboarded: false), "the launch screen waits")
        #expect(!MainWindowSwitch.opensFirstRun(ready: true, onboarded: true))
        #expect(MainUIRoute.onboarding.webPath == "/onboarding")
    }

    /// A launch (or a window that opens again) while the first run is not
    /// done asks for it as due, so the page can keep a "Skip for now": the
    /// welcome came back at every launch, in Cua Spaces Local as in the
    /// released app, because the app opened `/onboarding` itself. A person
    /// asking for it (Settings, Welcome) still gets the page unconditionally.
    @Test func aWindowOpeningWhileTheFirstRunIsDueLetsThePageHonourSkipForNow() {
        #expect(MainUIRoute.firstRunDue.webPath == "/onboarding?due=1")
        #expect(MainWindowSwitch.firstRunRoute(onboarded: false) == .firstRunDue)
        #expect(MainWindowSwitch.firstRunRoute(onboarded: true) == nil, "done: the app as it was")
        #expect(MainUIRoute.onboarding.webPath == "/onboarding", "Show again has no due: it always shows")
    }

    /// A window that is never shown (the replacement only reads its frame
    /// and closes it).
    func hiddenWindow() -> NSWindow {
        let w = NSWindow(contentRect: NSRect(x: 40, y: 40, width: 760, height: 480),
                         styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: true)
        w.isReleasedWhenClosed = false
        return w
    }

    /// First run, New UI on or off: the native window that appeared is
    /// replaced in its frame by the web window, never drawn.
    @Test func theNativeWindowOnAFirstRunIsReplacedInPlace() async {
        for webUi in [false, true] {
            let (_, model, _, _) = bridge(webUi: webUi)
            let w = hiddenWindow()
            let frame = w.frame
            var opened: [NSRect?] = []
            MainWindowSwitch.firstRunAppeared(w, model: model, web: { opened.append($0) })
            // Reported twice (its reader, its onAppear): replaced once.
            MainWindowSwitch.firstRunAppeared(nil, model: model, web: { opened.append($0) })
            #expect(opened == [frame], "New UI \(webUi ? "on" : "off")")
            await yield()
            await yield()
            #expect(w.alphaValue == 1, "hidden before it draws, then back to normal")
        }
    }

    /// Once the first run is done the native window is left alone.
    @Test func theNativeWindowStaysOnceTheFirstRunIsDone() async {
        let (_, model, _, _) = bridge()
        model.onboarding.finish()
        let w = hiddenWindow()
        var opened = 0
        MainWindowSwitch.firstRunAppeared(w, model: model, web: { _ in opened += 1 })
        await yield()
        #expect(opened == 0 && w.alphaValue == 1)
    }

    // MARK: - onboarding.get, onboarding.complete

    @Test func theFirstRunIsDueUntilItFinishesAndPersists() async throws {
        let (bridge, model, _, state) = bridge()
        let due = try #require(try await bridge.handle("onboarding.get", [:]) as? [String: Any])
        #expect(due["completed"] as? Bool == false && due["mode"] as? String == "client")
        let session = try #require(try await bridge.handle("session.get", [:]) as? [String: Any])
        #expect((session["onboarding"] as? [String: Any])?["completed"] as? Bool == false)

        let done = try #require(try await bridge.handle("onboarding.complete", ["mode": "host"]) as? [String: Any])
        #expect(done["completed"] as? Bool == true && done["mode"] as? String == "host")
        #expect(model.onboarding.completed)
        #expect(try String(contentsOf: state, encoding: .utf8) == #"{"completed":true,"mode":"host"}"#)
        let again = try #require(try await bridge.handle("onboarding.get", [:]) as? [String: Any])
        #expect(again["completed"] as? Bool == true && again["mode"] as? String == "host")
        #expect(((try await bridge.handle("session.get", [:]) as? [String: Any])?["onboarding"] as? [String: Any])?["completed"] as? Bool == true)
        // Settings, Welcome, shows it again.
        model.onboarding.restart()
        #expect((try await bridge.handle("onboarding.get", [:]) as? [String: Any])?["completed"] as? Bool == false)
    }

    @Test func completingNeedsAModeAndAppliesTheLoginCheckbox() async throws {
        let item = FixtureLoginItem()
        let (bridge, model, _, _) = bridge(loginItem: item)
        for bad: [String: Any] in [[:], ["mode": "team"], ["mode": 1]] {
            do {
                _ = try await bridge.handle("onboarding.complete", bad)
                Issue.record("expected bad_args for \(bad)")
            } catch let f as WebUIBridge.Failure {
                #expect(f.code == "bad_args")
            }
        }
        #expect(!model.onboarding.completed)
        _ = try await bridge.handle("onboarding.complete", ["mode": "client", "launchAtLogin": true])
        #expect(item.calls == ["register"])
        #expect(model.settings.launchAtLogin == true)
    }

    /// With New UI off the web page stood in for the first run: its window
    /// gives way to the native one. With it on, the page carries on in the
    /// same window.
    @Test func completingHandsOverToTheNativeWindowOnlyWithNewUIOff() async throws {
        for webUi in [false, true] {
            let (bridge, model, _, _) = bridge(webUi: webUi)
            var closed = 0
            model.closeWebUI = { closed += 1 }
            _ = try await bridge.handle("onboarding.complete", ["mode": "client"])
            #expect(closed == (webUi ? 0 : 1), "New UI \(webUi ? "on" : "off")")
        }
    }

    // MARK: - The usage notice

    @Test func theUsageNoticeIsSeenWhenWelcomeIsLeftNotBefore() async throws {
        let (bridge, _, telemetry, _) = bridge()
        func track(_ page: String, _ action: String) async throws {
            _ = try await bridge.handle("telemetry.track", ["signals": [
                ["type": "onboarding-page", "page": page, "action": action, "choice": "none"]]])
        }
        try await track("welcome", "shown")
        #expect(telemetry.acknowledgements == 0, "Welcome showing is not Welcome left")
        try await track("signin", "shown")
        #expect(telemetry.acknowledgements == 1)

        // Skipping the first run also moves past it.
        let (skip, _, skipped, _) = self.bridge()
        _ = try await skip.handle("telemetry.track", ["signals": [["type": "step", "step": "onboarding_skipped", "ok": true]]])
        #expect(skipped.acknowledgements == 1)

        // The switch beside the note, and finishing, count as seen too.
        let (settings, _, viaSwitch, _) = self.bridge()
        _ = try await settings.handle("settings.choose", ["row": "telemetry", "option": "off"])
        #expect(viaSwitch.acknowledgements == 1 && viaSwitch.current.enabled == false)
        let (finish, _, finished, _) = self.bridge()
        _ = try await finish.handle("onboarding.complete", ["mode": "client"])
        #expect(finished.acknowledgements == 1)
    }

    // MARK: - "<Space> is ready"

    @Test func theNotificationSaysWhatIsReadyAndWhereAClickGoes() {
        let linux = SpaceReadyNote(spaceId: "local:aurora", name: "aurora", os: .linux)
        #expect(linux.title == "aurora is ready")
        #expect(linux.body == "A fresh Linux computer inside this Mac. Your agent can use it now.")
        #expect(SpaceReadyNote(spaceId: "x", name: "x", os: .macos).body.hasPrefix("A fresh macOS computer"))
        #expect(SpaceReadyNote(spaceId: "x", name: "x", os: .windows).body.hasPrefix("A fresh Windows computer"))
        #expect(SpaceReadyNote(spaceId: "x", name: "x", os: .unknown).body
                == "A fresh computer inside this Mac. Your agent can use it now.")
        #expect(linux.identifier == "space-ready-local:aurora")
        #expect(SpaceReadyNote.spaceId(fromIdentifier: linux.identifier) == "local:aurora")
        #expect(SpaceReadyNote.spaceId(fromIdentifier: "agent-1") == nil)
        #expect(SpaceReadyNote.spaceId(fromIdentifier: "space-ready-") == nil)
    }

    @Test func theCuaKeyIsAttachedAsACopy() throws {
        let key = try #require(CuaKey.url, "the key is in the module's resources")
        #expect(CuaKey.image != nil)
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("cua-key-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let copy = try #require(SpaceReadyNotifier.attachmentCopy(of: key, in: dir))
        #expect(copy != key && copy.pathExtension == "png")
        #expect(try Data(contentsOf: copy) == Data(contentsOf: key))
        // The system moves the attachment's file: the original stays.
        try FileManager.default.removeItem(at: copy)
        #expect(FileManager.default.fileExists(atPath: key.path))
        #expect(SpaceReadyNotifier.attachmentCopy(of: nil, in: dir) == nil)
    }

    func config(_ name: String) -> [String: Any] {
        ["image": "ghcr.io/trycua/linux:24.04", "on": "local", "kind": "container", "runtime": "auto", "name": name]
    }

    @Test func aCreateFromTheOnboardingNotifiesWhenItSucceeds() async throws {
        let (bridge, _, _, _) = bridge()
        var posted: [SpaceReadyNote] = []
        bridge.notifySpaceReady = { posted.append($0) }
        _ = try await bridge.handle("spaces.create", ["config": config("aurora"), "pendingId": "pending:a1",
                                                       "os": "linux", "notifyWhenReady": true])
        #expect(posted == [SpaceReadyNote(spaceId: "local:aurora", name: "aurora", os: .linux)])
        // Any other create, and a flag that is not true, stay quiet.
        _ = try await bridge.handle("spaces.create", ["config": config("quiet"), "pendingId": "pending:a2", "os": "linux"])
        _ = try await bridge.handle("spaces.create", ["config": config("no"), "pendingId": "pending:a3",
                                                       "os": "linux", "notifyWhenReady": false])
        _ = try await bridge.handle("spaces.create", ["config": config("odd"), "pendingId": "pending:a4",
                                                       "os": "linux", "notifyWhenReady": "yes", "something": 1])
        #expect(posted.count == 1)
    }

    @Test func aFailedOrCancelledCreateNeverNotifies() async throws {
        let backend = FixtureSpacesBackend()
        backend.cancelledCreates = ["pending:b1"]
        let (bridge, _, _, _) = bridge(backend: backend)
        var posted: [SpaceReadyNote] = []
        bridge.notifySpaceReady = { posted.append($0) }
        for id in ["pending:b1"] {
            do {
                _ = try await bridge.handle("spaces.create", ["config": config("gone"), "pendingId": id,
                                                               "os": "linux", "notifyWhenReady": true])
                Issue.record("expected cancelled")
            } catch let f as WebUIBridge.Failure {
                #expect(f.code == "cancelled")
            }
        }
        backend.createError = "no room"
        do {
            _ = try await bridge.handle("spaces.create", ["config": config("full"), "pendingId": "pending:b2",
                                                           "os": "linux", "notifyWhenReady": true])
            Issue.record("expected a failure")
        } catch {}
        #expect(posted.isEmpty)
    }

    /// The create belongs to the app: closing the onboarding window mid-create
    /// neither cancels it nor loses the notification.
    @Test func closingTheWindowMidCreateDoesNotCancelIt() async throws {
        let backend = FixtureSpacesBackend()
        backend.holdCreates = true
        let (_, model, _, _) = bridge(backend: backend)
        let controller = WebUIWindowController(model: model, route: "/onboarding")
        var posted: [SpaceReadyNote] = []
        controller.bridge.notifySpaceReady = { posted.append($0) }
        let create = Task { @MainActor in
            try await controller.bridge.handle("spaces.create", ["config": config("aurora"), "pendingId": "pending:c1",
                                                                  "os": "linux", "notifyWhenReady": true])
        }
        for _ in 0..<200 where model.creates.pending.isEmpty { await yield() }
        #expect(model.creates.pending.map(\.id) == ["pending:c1"])
        // The window goes (as the red button does); its page and bridge with it.
        controller.window?.close()
        await yield()
        #expect(model.creates.pending.map(\.id) == ["pending:c1"], "still creating")
        backend.releaseCreate()
        let space = try #require(try await create.value as? [String: Any])
        #expect(space["id"] as? String == "local:aurora")
        #expect(model.creates.pending.isEmpty)
        #expect(backend.created.count == 1)
        #expect(posted == [SpaceReadyNote(spaceId: "local:aurora", name: "aurora", os: .linux)])
    }
}

extension FirstRunTests {
    /// `session.openExternal` is `window.open` on this host: web pages, mail
    /// and System Settings' panes (a permission row's link) reach the system;
    /// nothing else does.
    @Test func theWindowHandsOnlyPagesMailAndSystemSettingsToTheSystem() {
        for ok in ["https://cua.ai/docs", "http://localhost:3000", "mailto:hi@cua.ai",
                   "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"] {
            #expect(WebUIWindowController.opensOutside(URL(string: ok)!), "\(ok)")
        }
        for no in ["file:///etc/hosts", "cua-spaces://app/settings", "javascript:alert(1)", "tel:123", "x-apple.systempreferences"] {
            #expect(!WebUIWindowController.opensOutside(URL(string: no)!), "\(no)")
        }
    }
}
