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

    /// The first run's step, the Driver page and This Mac read the same two
    /// rows (`driver.permissions`), each with the pane its Allow guides.
    @Test func thePagesReadBothPermissionsWithTheirPanes() throws {
        let rows = PermissionGuide.rows(states: [.screenRecording: .off, .accessibility: .on])
        #expect(rows.map { $0["id"] as? String } == ["screen", "accessibility"])
        #expect(rows.map { $0["granted"] as? Bool } == [false, true])
        #expect(rows.map { $0["state"] as? String } == ["off", "on"])
        for row in rows {
            let url = try #require(row["settingsUrl"] as? String)
            #expect(PermissionGuide.pane(for: url) != nil)
        }
        #expect(PermissionGuide.rows(states: [:]).allSatisfy { $0["granted"] as? Bool == false && $0["state"] as? String == "unknown" })
    }

    @Test func namesTheAppSystemSettingsLists() {
        #expect(PermissionGuide.appName(info: ["CFBundleDisplayName": "Cua Spaces Local", "CFBundleName": "CuaSpacesMac"]) == "Cua Spaces Local")
        #expect(PermissionGuide.appName(info: ["CFBundleName": "Cua Spaces"]) == "Cua Spaces")
        #expect(PermissionGuide.appName(info: ["CFBundleDisplayName": ""]) == "Cua Spaces")
        #expect(PermissionGuide.appName(info: nil) == "Cua Spaces")
    }

    @Test func everyGuidedPaneHasItsSettingsURL() {
        for pane in PermissionGuide.guided {
            let url = PermissionGuide.settingsURL(for: pane)
            #expect(url.flatMap(PermissionGuide.pane(for:)) == pane)
        }
    }

    @MainActor
    /// TCC's preflight: 0 allowed, 1 a row that says no, 2 no row. The
    /// public call decides "on".
    @Test func tellsNoRowFromARowSwitchedOffFromOn() {
        #expect(PermissionGuide.paneState(granted: true, tcc: 1) == .on)
        #expect(PermissionGuide.paneState(granted: true, tcc: nil) == .on)
        #expect(PermissionGuide.paneState(granted: false, tcc: 1) == .off)
        #expect(PermissionGuide.paneState(granted: false, tcc: 2) == .absent)
        // A row that says yes for another build (signed differently): its
        // switch looks on, but this copy has no grant.
        #expect(PermissionGuide.paneState(granted: false, tcc: 0) == .off)
        #expect(PermissionGuide.paneState(granted: false, tcc: nil) == .unknown)
        #expect(PermissionGuide.paneState(granted: false, tcc: 7) == .unknown)
    }

    /// Every state gets the panel under System Settings, with its own words
    /// (CUA-3551: a listed-but-off app got only a banner at the top).
    @Test func thePanelSaysWhatThePaneNeeds() {
        let name = "Cua Spaces Local"
        let absent = PermissionGuide.guidance(for: .screenRecording, state: .absent, appName: name, paneName: "Screen Recording")
        #expect(absent.title == "Drag **Cua Spaces Local** into the list above to allow **Screen Recording**.")
        #expect(absent.showsApp)
        let off = PermissionGuide.guidance(for: .accessibility, state: .off, appName: name, paneName: "Accessibility")
        #expect(off.title == "Turn on the switch next to **Cua Spaces Local** in the list above.")
        #expect(off.showsApp, "the tile stays for a row an older build left")
        // A rebuilt Cua Spaces Local: its old row looks on, but this copy has no grant.
        #expect(off.detail == "Already on, or not in the list? Drag this in.")
        let unknown = PermissionGuide.guidance(for: .accessibility, state: .unknown, appName: name, paneName: "Accessibility")
        #expect(unknown.showsApp)
        #expect(unknown.detail == "Not in the list? Drag this in.")
        let on = PermissionGuide.guidance(for: .screenRecording, state: .on, appName: name, paneName: "Screen Recording")
        #expect(on.title == "**Screen Recording** is on for **Cua Spaces Local**.")
        #expect(!on.showsApp)
        #expect(on.detail?.contains("Quit & Reopen") == true)
        #expect(PermissionGuide.guidance(for: .accessibility, state: .on, appName: name, paneName: "Accessibility").detail == nil)
        for state in [PermissionGuide.PaneState.absent, .off, .unknown, .on] {
            let words = PermissionGuide.guidance(for: .accessibility, state: state, appName: name, paneName: "Accessibility")
            for jargon in ["TCC", "AX", "daemon", "database"] { #expect(!words.title.contains(jargon)) }
        }
    }

    @Test func readsWhatAFreshProcessReports() {
        #expect(PermissionGuide.parseStates("screen-recording=on accessibility=absent\n") == [.screenRecording: .on, .accessibility: .absent])
        #expect(PermissionGuide.parseStates("screen-recording=off accessibility=on") == [.screenRecording: .off, .accessibility: .on])
        // An older build's answer.
        #expect(PermissionGuide.parseStates("screen-recording=1 accessibility=0") == [.screenRecording: .on, .accessibility: .unknown])
        #expect(PermissionGuide.parseStates("") == [.screenRecording: .unknown, .accessibility: .unknown])
        #expect(PermissionGuide.parseStates("garbage=1 accessibility=maybe") == [.screenRecording: .unknown, .accessibility: .unknown])
        #expect(PermissionGuide.granted(in: [.screenRecording: .on, .accessibility: .off]) == [.screenRecording])
    }

    /// This process answers in the words a fresh copy prints.
    @Test func thePreflightReportParsesBack() {
        let report = PermissionGuide.preflightReport()
        #expect(report.hasPrefix("screen-recording="))
        #expect(PermissionGuide.parseStates(report).count == 2)
    }

    /// After "Quit & Reopen" the guide comes back on what is still missing,
    /// for 15 minutes.
    @Test func picksTheGuideUpAgainAfterARelaunch() {
        let saved = PermissionGuide.resumeStore
        defer { PermissionGuide.resumeStore = saved }
        let suite = "PermissionGuideTests.resume.\(UUID().uuidString)"
        let store = UserDefaults(suiteName: suite)!
        defer { store.removePersistentDomain(forName: suite) }
        PermissionGuide.resumeStore = store
        let t0 = Date(timeIntervalSince1970: 1_000_000)
        #expect(PermissionGuide.resumePane(now: t0, states: [:]) == nil)
        PermissionGuide.noteGuiding(.screenRecording, at: t0)
        // Screen Recording still off: guide it again.
        #expect(PermissionGuide.resumePane(now: t0 + 30, states: [.screenRecording: .off, .accessibility: .absent]) == .screenRecording)
        // On after the reopen: on to Accessibility.
        #expect(PermissionGuide.resumePane(now: t0 + 30, states: [.screenRecording: .on, .accessibility: .absent]) == .accessibility)
        // Both on: nothing.
        #expect(PermissionGuide.resumePane(now: t0 + 30, states: [.screenRecording: .on, .accessibility: .on]) == nil)
        // An old note: nothing.
        #expect(PermissionGuide.resumePane(now: t0 + 16 * 60, states: [.screenRecording: .off]) == nil)
        PermissionGuide.clearGuiding()
        #expect(PermissionGuide.resumePane(now: t0 + 30, states: [.screenRecording: .off]) == nil)
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
             PermissionGuide.readStates, PermissionGuide.resumeStore)
        defer {
            (PermissionGuide.openPane, PermissionGuide.showsGuide, PermissionGuide.grantedHook,
             PermissionGuide.readStates, PermissionGuide.resumeStore) = (savedOpen, savedShows, savedHook, savedRead, savedStore)
        }
        let suite = "PermissionGuideTests.bridge.\(UUID().uuidString)"
        let store = UserDefaults(suiteName: suite)!
        defer { store.removePersistentDomain(forName: suite) }
        PermissionGuide.resumeStore = store
        var opened: [String] = []
        var granted: [PermissionFlowPane] = []
        PermissionGuide.openPane = { opened.append($0.absoluteString) }
        PermissionGuide.showsGuide = false
        PermissionGuide.grantedHook = { granted.append($0) }
        // Not on for the first reads (no row, or a row switched off); then
        // the person turns it on.
        actor Reads {
            var n = 0
            var before: PermissionGuide.PaneState = .absent
            func next() -> [PermissionFlowPane: PermissionGuide.PaneState] {
                n += 1
                return [.screenRecording: .absent, .accessibility: n > 2 ? .on : before]
            }
            func reset(_ state: PermissionGuide.PaneState) { n = 0; before = state }
        }
        let reads = Reads()
        PermissionGuide.readStates = { await reads.next() }

        // A switched-off (listed) app and a never-asked one both go to the pane.
        for before in [PermissionGuide.PaneState.off, .absent] {
            await reads.reset(before)
            opened = []
            granted = []
            let url = "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
            _ = try await bridge.handle("host.openSettings", ["url": url])
            #expect(opened == [url], "before: \(before)")
            #expect(store.dictionary(forKey: PermissionGuide.resumeKey)?["pane"] as? String == PermissionFlowPane.accessibility.rawValue)
            for _ in 0..<200 where granted.isEmpty { try await Task.sleep(for: .milliseconds(50)) }
            #expect(granted == [.accessibility], "before: \(before)")
        }

        // The pages read the live state on every call.
        let answer = try await bridge.handle("driver.permissions", [:]) as? [String: Any]
        let rows = try #require(answer?["permissions"] as? [[String: Any]])
        #expect(rows.map { $0["state"] as? String } == ["absent", "on"])
    }

    /// WKWebView refuses a window's first click by default: with System
    /// Settings in front, "Allow" then only brought the app forward.
    @MainActor
    @Test func theWebViewTakesTheClickThatActivatesItsWindow() {
        let web = WebUIWebView(frame: .zero, configuration: WKWebViewConfiguration())
        #expect(web.acceptsFirstMouse(for: nil))
    }
}

/// Where the guide panel docks (CUA-3551). Mac test, 7 Nov: on a 1024x768
/// screen with a 60 pt Dock, the panel sat under System Settings with its
/// drag row in view on the first grant, and about 60 pt lower, its drag row
/// behind the Dock, after a relaunch, when Cua Spaces came back full screen:
/// a full-screen app's `visibleFrame` is its own Space's, with no Dock.
struct GuidePanelPlacementTests {
    let screenFrame = CGRect(x: 0, y: 0, width: 1024, height: 768)
    /// The desktop Space: a 60 pt Dock at the bottom, a 24 pt menu bar.
    var desktop: PanelScreen { PanelScreen(frame: screenFrame, visibleFrame: CGRect(x: 0, y: 60, width: 1024, height: 684)) }
    /// What a full-screen Cua Spaces reads: neither the Dock nor the menu bar.
    var fullScreen: PanelScreen { PanelScreen(frame: screenFrame, visibleFrame: screenFrame) }
    /// System Settings as in the shots: its bottom edge just above the Dock.
    let settings = CGRect(x: 126, y: 66, width: 770, height: 668)
    let panelHeight: CGFloat = 120

    func docked(_ screen: PanelScreen, hostIsFullScreen: Bool) -> CGRect {
        let usable = PanelPlacement.usableArea(screen: screen, settingsFrame: settings,
                                               hostIsFullScreen: hostIsFullScreen, menuBarHeight: 24)
        return PanelPlacement.dockedFrame(settingsFrame: settings, usable: usable, height: { _ in panelHeight })
    }

    @Test func staysClearOfTheDockOnTheDesktop() {
        let panel = docked(desktop, hostIsFullScreen: false)
        #expect(panel.minY >= 60 + PanelPlacement.screenInset)
        #expect(panel.minX == settings.minX + PanelPlacement.sidebarWidth)
        #expect(panel.width == settings.width - PanelPlacement.sidebarWidth)
    }

    /// The bug: the same System Settings window, asked from a full-screen
    /// Cua Spaces, must give the same, fully visible panel.
    @Test func staysClearOfTheDockWhenTheAppIsFullScreen() {
        let first = docked(desktop, hostIsFullScreen: false)
        // macOS may or may not report the full-screen flag before the
        // visible frame changes: either one alone is enough.
        for (screen, flag) in [(fullScreen, true), (fullScreen, false), (desktop, true)] {
            let panel = docked(screen, hostIsFullScreen: flag)
            #expect(panel.minY >= 60, "the drag row must not sit behind the Dock")
            #expect(panel.minY >= settings.minY)
            #expect(panel.maxY <= screenFrame.maxY - 24)
            #expect(abs(panel.minY - first.minY) < 8)
        }
    }

    /// With room under System Settings, the panel sits right under it.
    @Test func sitsRightUnderAShortWindow() {
        let short = CGRect(x: 126, y: 400, width: 770, height: 340)
        let usable = PanelPlacement.usableArea(screen: desktop, settingsFrame: short, hostIsFullScreen: false, menuBarHeight: 24)
        let panel = PanelPlacement.dockedFrame(settingsFrame: short, usable: usable, height: { _ in panelHeight })
        #expect(panel.maxY == short.minY)
    }

    /// Picks the screen System Settings is mostly on.
    @Test func picksTheScreenSystemSettingsIsOn() {
        let right = PanelScreen(frame: CGRect(x: 1024, y: 0, width: 1440, height: 900),
                                visibleFrame: CGRect(x: 1024, y: 0, width: 1440, height: 875))
        let onRight = CGRect(x: 900, y: 100, width: 770, height: 600)
        #expect(PanelPlacement.screen(for: onRight, among: [desktop, right]) == right)
        #expect(PanelPlacement.screen(for: settings, among: [desktop, right]) == desktop)
        #expect(PanelPlacement.screen(for: CGRect(x: 5000, y: 0, width: 10, height: 10), among: [desktop, right]) == nil)
    }
}
