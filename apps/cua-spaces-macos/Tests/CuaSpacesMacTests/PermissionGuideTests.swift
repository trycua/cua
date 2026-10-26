// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesMacKit
import Foundation
import PermissionFlow
import Testing
import WebKit

/// `host.openSettings` guides Screen Recording and Accessibility (the panes
/// the host's permission hints and the first run's rows name); any other
/// pane just opens.
struct PermissionGuideTests {
    @Test func guidesTheTwoPanesASharedScreenNeeds() {
        #expect(PermissionGuide.pane(for: "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture") == .screenRecording)
        #expect(PermissionGuide.pane(for: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility") == .accessibility)
        #expect(PermissionGuide.pane(for: "x-apple.systempreferences:com.apple.LoginItems-Settings.extension") == nil)
        #expect(PermissionGuide.pane(for: "x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork") == nil)
    }

    @Test func offersTheOtherPermissionNextWhileItIsMissing() {
        #expect(PermissionGuide.next(after: .screenRecording, isGranted: { _ in false }) == .guide(.accessibility))
        #expect(PermissionGuide.next(after: .accessibility, isGranted: { _ in false }) == .guide(.screenRecording))
    }

    @Test func finishesWhenTheOtherPermissionIsAlreadyGranted() {
        #expect(PermissionGuide.next(after: .screenRecording, isGranted: { $0 == .accessibility }) == .finish)
        #expect(PermissionGuide.next(after: .accessibility, isGranted: { $0 == .screenRecording }) == .finish)
    }

    @Test func onlyScreenRecordingNeedsAReopen() {
        #expect(PermissionGuide.needsReopen(after: .screenRecording))
        #expect(!PermissionGuide.needsReopen(after: .accessibility))
    }

    @Test func everyGuidedPaneHasItsSettingsURL() {
        for pane in PermissionGuide.guided {
            let url = PermissionGuide.settingsURL(for: pane)
            #expect(url.flatMap(PermissionGuide.pane(for:)) == pane)
        }
    }

    @MainActor
    @Test func saysSwitchOnWhenTheAppIsListedAndDragWhenItIsNot() {
        let saved = PermissionGuide.listedStore
        defer { PermissionGuide.listedStore = saved }
        let suite = "PermissionGuideTests.\(UUID().uuidString)"
        let store = UserDefaults(suiteName: suite)!
        defer { store.removePersistentDomain(forName: suite) }
        PermissionGuide.listedStore = store

        #expect(!PermissionGuide.isListed(.accessibility))
        #expect(PermissionGuide.instruction(for: .accessibility, listed: PermissionGuide.isListed(.accessibility)) == .drag)
        PermissionGuide.markListed(.accessibility)
        #expect(PermissionGuide.isListed(.accessibility))
        #expect(!PermissionGuide.isListed(.screenRecording))
        #expect(PermissionGuide.instruction(for: .accessibility, listed: PermissionGuide.isListed(.accessibility)) == .switchOn)
        PermissionGuide.markListed(.accessibility, false)
        #expect(PermissionGuide.instruction(for: .accessibility, listed: PermissionGuide.isListed(.accessibility)) == .drag)
        #expect(PermissionGuide.switchOnText(appName: "Cua Spaces Local") == "Turn on Cua Spaces Local in System Settings, then come back.")
    }

    @Test func readsWhatAFreshProcessReports() {
        #expect(PermissionGuide.parseGrants("screen-recording=1 accessibility=0\n") == [.screenRecording])
        #expect(PermissionGuide.parseGrants("screen-recording=0 accessibility=1") == [.accessibility])
        #expect(PermissionGuide.parseGrants("screen-recording=1 accessibility=1") == [.screenRecording, .accessibility])
        #expect(PermissionGuide.parseGrants("").isEmpty)
        #expect(PermissionGuide.parseGrants("garbage=1").isEmpty)
    }

    @Test func tellsNotAskedFromSwitchedOffFromGranted() {
        #expect(PermissionGuide.state(granted: false, listed: false) == .notDetermined)
        #expect(PermissionGuide.state(granted: false, listed: true) == .off)
        #expect(PermissionGuide.state(granted: true, listed: true) == .granted)
        // An off permission is guided by opening its pane, never by a prompt.
        #expect(PermissionGuide.instruction(for: .screenRecording, listed: true) == .switchOn)
    }

    @Test func deniedThenSettingsOpenedThenGrantedCompletesWithoutAnotherClick() async {
        // Off for three polls, then the person turns it on in System Settings.
        actor Reads { var n = 0; func next() -> Set<PermissionFlowPane> { n += 1; return n > 3 ? [.screenRecording] : [] } }
        let reads = Reads()
        let done = await PermissionGuide.waitForGrant(.screenRecording, read: { await reads.next() },
                                                      interval: .milliseconds(1), giveUpAfter: .seconds(5))
        #expect(done)
        #expect(PermissionGuide.needsReopen(after: .screenRecording))
        #expect(PermissionGuide.settingsURL(for: .screenRecording)?.hasSuffix("Privacy_ScreenCapture") == true)
    }

    @Test func staysUndoneWhileItIsOff() async {
        let done = await PermissionGuide.waitForGrant(.accessibility, read: { [] },
                                                      interval: .milliseconds(1), giveUpAfter: .milliseconds(30))
        #expect(!done)
    }

    /// The page's Allow reaches System Settings through the bridge, and a
    /// grant after that ends the watch (CUA-2342).
    @MainActor
    @Test func allowOpensThePaneThroughTheBridgeThenAGrantCompletes() async throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("cua-allow-\(UUID().uuidString)")
        let model = AppModel(backend: FixtureSpacesBackend(rows: []), keyvault: KeyvaultModel(client: nil),
                             onboarding: OnboardingModel(statePath: dir.appendingPathComponent("onboarding.json").path),
                             settingsPath: dir.appendingPathComponent("settings.json").path,
                             telemetry: FixtureTelemetry())
        let bridge = WebUIBridge(model: model, allowedOrigins: [])
        let (savedOpen, savedShows, savedHook, savedRead, savedStore) =
            (PermissionGuide.openPane, PermissionGuide.showsGuide, PermissionGuide.grantedHook,
             PermissionGuide.readGranted, PermissionGuide.listedStore)
        defer {
            (PermissionGuide.openPane, PermissionGuide.showsGuide, PermissionGuide.grantedHook,
             PermissionGuide.readGranted, PermissionGuide.listedStore) = (savedOpen, savedShows, savedHook, savedRead, savedStore)
        }
        let suite = "PermissionGuideTests.bridge.\(UUID().uuidString)"
        let store = UserDefaults(suiteName: suite)!
        defer { store.removePersistentDomain(forName: suite) }
        PermissionGuide.listedStore = store
        var opened: [String] = []
        var granted: [PermissionFlowPane] = []
        PermissionGuide.openPane = { opened.append($0.absoluteString) }
        PermissionGuide.showsGuide = false
        PermissionGuide.grantedHook = { granted.append($0) }
        // Denied: off for the first reads; then the person turns it on.
        actor Reads {
            var n = 0
            func next() -> Set<PermissionFlowPane> { n += 1; return n > 2 ? [.accessibility] : [] }
            func reset() { n = 0 }
        }
        let reads = Reads()
        PermissionGuide.readGranted = { await reads.next() }

        // Both a switched-off (listed) and a never-asked app go to the pane.
        for listed in [true, false] {
            opened = []
            granted = []
            PermissionGuide.markListed(.accessibility, listed)
            let url = "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
            _ = try await bridge.handle("host.openSettings", ["url": url])
            #expect(opened == [url], "listed: \(listed)")
            for _ in 0..<200 where granted.isEmpty { try await Task.sleep(for: .milliseconds(50)) }
            #expect(granted == [.accessibility], "listed: \(listed)")
            await reads.reset()
        }
    }

    /// WKWebView refuses a window's first click by default: with System
    /// Settings in front, "Allow" then only brought the app forward.
    @MainActor
    @Test func theWebViewTakesTheClickThatActivatesItsWindow() {
        let web = WebUIWebView(frame: .zero, configuration: WKWebViewConfiguration())
        #expect(web.acceptsFirstMouse(for: nil))
    }
}
