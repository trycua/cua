// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import ApplicationServices
import CoreGraphics
import PermissionFlow

/// The app's own guide to a privacy pane of System Settings, so nobody
/// meets macOS's bare "allow?" prompt first. PermissionFlow (MIT,
/// github.com/jaywcjlove/PermissionFlow, vendored in Vendor/PermissionFlow)
/// opens the pane and docks a panel under System Settings. The panel always
/// shows, and says what the pane needs: drag the app into the list when it
/// has no row, turn its switch on when it has one, then that it is on. It
/// reads the pane every second, and picks up again after "Quit & Reopen".
/// The app's name and icon are this bundle's, so Cua Spaces Local shows as
/// itself.
///
/// The first run's "Share this Mac's screen" and Settings' "Permissions"
/// reach it through `host.openSettings`: Screen Recording and Accessibility
/// are guided, every other pane just opens.
@MainActor
public enum PermissionGuide {
    /// A file beside the host service's spec (`<CuaHome>/helper/`), written
    /// once the app has run with its window on this Mac: from then on the
    /// app asks for both permissions itself, and the host service
    /// (ServiceLauncher) never shows macOS's own prompts. A Mac hosting from
    /// the CLI only, where the app never showed, keeps them. A file, not
    /// the app's defaults: the service reads it at launch without depending
    /// on how launchd's process resolves the preferences domain.
    public nonisolated static func markerPath(specPath: String) -> String {
        ((specPath as NSString).deletingLastPathComponent as NSString)
            .appendingPathComponent("permissions-asked-in-app")
    }

    public nonisolated static func appAsks(specPath: String) -> Bool {
        FileManager.default.fileExists(atPath: markerPath(specPath: specPath))
    }

    public nonisolated static func noteAppAsks(specPath: String) {
        let marker = markerPath(specPath: specPath)
        guard !FileManager.default.fileExists(atPath: marker) else { return }
        try? FileManager.default.createDirectory(
            atPath: (marker as NSString).deletingLastPathComponent, withIntermediateDirectories: true)
        FileManager.default.createFile(atPath: marker, contents: Data("1\n".utf8))
    }

    /// The pane a System Settings URL names, when the guide knows it.
    nonisolated static func pane(for url: String) -> PermissionFlowPane? {
        if url.contains("Privacy_ScreenCapture") { return .screenRecording }
        if url.contains("Privacy_Accessibility") { return .accessibility }
        return nil
    }

    private static var controller: PermissionFlowController?

    /// How a pane is opened, what the guide draws over it, and what runs on
    /// a grant. Replaced in tests, which must not open System Settings or
    /// draw panels.
    nonisolated(unsafe) static var openPane: (URL) -> Void = { NSWorkspace.shared.open($0) }
    nonisolated(unsafe) static var showsGuide = true
    nonisolated(unsafe) static var grantedHook: ((PermissionFlowPane) -> Void)?

    // MARK: - Where a permission stands (CUA-2283, CUA-3551)

    /// Where one permission stands in System Settings' list for this copy of
    /// the app. The guide's panel says something different for each, and the
    /// pages show it on the permission's row.
    public enum PaneState: String, Equatable, Sendable {
        /// Granted.
        case on
        /// The app has a row in the list, with its switch off.
        case off
        /// No row for this copy of the app: drag it in. A row left by an
        /// older build signed differently does not count (macOS ignores it).
        case absent
        /// This macOS doesn't say (the private check below is missing): off
        /// or absent.
        case unknown
    }

    /// The four states from what macOS reports: the public preflight call
    /// (`granted`) and TCC's own preflight (`tcc`: 0 allowed, 1 a row that
    /// says no, 2 no row; nil when the call is missing). The public call wins
    /// for "on": it is what the app's capture and input really use.
    nonisolated static func paneState(granted: Bool, tcc: Int32?) -> PaneState {
        if granted { return .on }
        switch tcc {
        // A row that says yes while the public call says no: its switch
        // shows on but belongs to another build; turning it on again or
        // dragging this copy in is the fix, as for a row that is off.
        case 0, 1: return .off
        case 2: return .absent
        default: return .unknown
        }
    }

    /// TCC's preflight (`TCCAccessPreflight` in the private TCC framework):
    /// the only call that tells "no row" from "a row switched off" without
    /// reading TCC's database (which needs Full Disk Access). Looked up at
    /// run time, so a macOS without it answers nil and the guide says both.
    nonisolated static func tccPreflight(_ pane: PermissionFlowPane) -> Int32? {
        let service: String
        switch pane {
        case .screenRecording: service = "kTCCServiceScreenCapture"
        case .accessibility: service = "kTCCServiceAccessibility"
        default: return nil
        }
        guard let preflight = tccPreflightCall else { return nil }
        return preflight(service as CFString, nil)
    }

    private typealias TCCPreflightFn = @convention(c) (CFString, CFDictionary?) -> Int32
    private nonisolated static let tccPreflightCall: TCCPreflightFn? = {
        guard let handle = dlopen("/System/Library/PrivateFrameworks/TCC.framework/TCC", RTLD_LAZY),
              let symbol = dlsym(handle, "TCCAccessPreflight") else { return nil }
        return unsafeBitCast(symbol, to: TCCPreflightFn.self)
    }()

    /// What the panel under System Settings says for `pane` in `state`:
    /// drag the app in, turn its switch on, or done. The app's tile stays
    /// while the switch is off too, for a row that belongs to an older build.
    nonisolated static func guidance(for pane: PermissionFlowPane, state: PaneState, appName: String,
                                     paneName: String) -> PermissionFlowGuidance {
        switch state {
        case .absent:
            return .init(title: "Drag **\(appName)** into the list above to allow **\(paneName)**.",
                         detail: "Then make sure its switch is on.", showsApp: true)
        case .off:
            return .init(title: "Turn on the switch next to **\(appName)** in the list above.",
                         detail: "Already on, or not in the list? Drag this in.",
                         symbol: "switch.2", showsApp: true)
        case .unknown:
            return .init(title: "Turn on **\(appName)** in the list above to allow **\(paneName)**.",
                         detail: "Not in the list? Drag this in.", symbol: "switch.2", showsApp: true)
        case .on:
            return .init(title: "**\(paneName)** is on for **\(appName)**.",
                         detail: needsReopen(after: pane) ? "If macOS asks, choose Quit & Reopen. \(appName) comes back where you were." : nil,
                         symbol: "checkmark.circle.fill", showsApp: false)
        }
    }

    /// The pane's name as System Settings shows it ("Screen & System Audio
    /// Recording" is listed under Screen Recording; Accessibility is "Device
    /// Control and Data Access" from macOS 27).
    nonisolated static func paneName(_ pane: PermissionFlowPane) -> String {
        pane.localizedTitle(localeIdentifier: nil)
    }

    /// Polls until `pane` reads as granted (true) or `deadline` passes (false),
    /// so the step completes without a second click.
    nonisolated static func waitForGrant(_ pane: PermissionFlowPane,
                                         read: () async -> Set<PermissionFlowPane>,
                                         interval: Duration, giveUpAfter: Duration) async -> Bool {
        let clock = ContinuousClock()
        let deadline = clock.now + giveUpAfter
        while !Task.isCancelled, clock.now < deadline {
            if await read().contains(pane) { return true }
            try? await Task.sleep(for: interval)
        }
        return false
    }

    // MARK: - Coming back after Quit & Reopen

    /// A short note that a guide is open, so a relaunch (macOS's "Quit &
    /// Reopen", or the app's own) picks the guide up again instead of
    /// leaving System Settings with no panel. Kept 15 minutes.
    nonisolated(unsafe) static var resumeStore: UserDefaults = .standard
    nonisolated static let resumeKey = "permissionGuide.resume"
    nonisolated static let resumeFor: TimeInterval = 15 * 60

    nonisolated static func noteGuiding(_ pane: PermissionFlowPane, at now: Date = Date()) {
        resumeStore.set(["pane": pane.rawValue, "at": now.timeIntervalSince1970], forKey: resumeKey)
    }

    nonisolated static func clearGuiding() { resumeStore.removeObject(forKey: resumeKey) }

    /// The pane to guide after a relaunch: the noted one while it is still
    /// missing, else the other one still missing; nil when the note is old
    /// or gone, or both are on.
    nonisolated static func resumePane(now: Date = Date(), states: [PermissionFlowPane: PaneState]) -> PermissionFlowPane? {
        guard let note = resumeStore.dictionary(forKey: resumeKey),
              let raw = note["pane"] as? String, let pane = PermissionFlowPane(rawValue: raw),
              let at = note["at"] as? TimeInterval,
              now.timeIntervalSince1970 - at < resumeFor, now.timeIntervalSince1970 >= at else { return nil }
        if states[pane] != .on { return pane }
        return guided.first { states[$0] != .on }
    }

    /// At launch: when a guide was open just before (and System Settings
    /// still is), show it again on what is still missing.
    public static func resumeAfterRelaunch() {
        Task { @MainActor in
            let states = await readStates()
            guard let pane = resumePane(states: states) else { return clearGuiding() }
            guard NSRunningApplication.runningApplications(withBundleIdentifier: "com.apple.systempreferences").first != nil,
                  let url = settingsURL(for: pane) else { return }
            guide(url)
        }
    }

    /// Guides `url`'s pane: false when the guide doesn't know it (open it
    /// plainly then). The panel docks under System Settings whatever the
    /// pane shows, and its words follow the pane: drag the app in, turn its
    /// switch on, done.
    @discardableResult
    public static func guide(_ url: String) -> Bool {
        guard let pane = pane(for: url) else { return false }
        noteGuiding(pane)
        guard showsGuide else {
            if let u = URL(string: url) { openPane(u) }
            watch(pane)
            return true
        }
        let app = Bundle.main.bundleURL
        let flow = controller ?? PermissionFlowController(configuration: .init(requiredAppURLs: [app]))
        controller = flow
        // Words for the state last read, until the first read of this guide.
        flow.setGuidance(guidance(for: pane, state: lastStates[pane] ?? .unknown, appName: appName(), paneName: paneName(pane)))
        // Where the click came from, for the panel's flight to System Settings.
        let from = NSApp.keyWindow.map { w -> CGRect in
            let mouse = NSEvent.mouseLocation
            return w.frame.contains(mouse) ? CGRect(x: mouse.x - 20, y: mouse.y - 10, width: 40, height: 20) : w.frame
        }
        flow.authorize(pane: pane, suggestedAppURLs: [app], sourceFrameInScreen: from)
        watch(pane)
        return true
    }

    // MARK: - Closing itself once access is granted (CUA-2274)

    /// PermissionFlow 2.11.4 has no completion callback: its panel stays on
    /// System Settings until closed. So the guide watches for the grant and
    /// closes the panel the moment its pane is granted.
    ///
    /// The check runs in a fresh copy of this executable
    /// (`preflight-permissions`, main.swift), the way the permission rows
    /// are answered: a running process keeps the Screen Recording answer it
    /// had at launch until it reopens, so asking this process would never
    /// see the grant. Replaceable in tests.
    nonisolated(unsafe) static var readStates: () async -> [PermissionFlowPane: PaneState] = {
        await Task.detached { freshProcessStates() }.value
    }

    /// The guided panes macOS has granted this app (from `readStates`).
    static func readGranted() async -> Set<PermissionFlowPane> {
        granted(in: await readStates())
    }

    nonisolated static func granted(in states: [PermissionFlowPane: PaneState]) -> Set<PermissionFlowPane> {
        Set(states.filter { $0.value == .on }.keys)
    }

    /// The states read last, for the guide's first words before its own read.
    private static var lastStates: [PermissionFlowPane: PaneState] = [:]

    /// `CuaSpacesMac preflight-permissions` in a new process: where each
    /// guided pane stands for this app.
    nonisolated static func freshProcessStates() -> [PermissionFlowPane: PaneState] {
        // Not running as an app (tests, `swift run`): this process is the
        // one macOS answers for, and there is no app to start a copy of.
        guard Bundle.main.bundleURL.pathExtension == "app" else { return parseStates(preflightReport()) }
        guard let exe = Bundle.main.executableURL else { return [:] }
        let p = Process()
        p.executableURL = exe
        p.arguments = ["preflight-permissions"]
        let out = Pipe()
        p.standardOutput = out
        p.standardError = FileHandle.nullDevice
        guard (try? p.run()) != nil else { return [:] }
        let data = out.fileHandleForReading.readDataToEndOfFile()
        p.waitUntilExit()
        return parseStates(String(decoding: data, as: UTF8.self))
    }

    /// What `preflight-permissions` prints: `screen-recording=on
    /// accessibility=absent` (`on`, `off`, `absent`, `unknown`; an older
    /// build's `1` and `0` read as on and unknown). A pane it doesn't
    /// mention is unknown.
    nonisolated static func parseStates(_ line: String) -> [PermissionFlowPane: PaneState] {
        var states: [PermissionFlowPane: PaneState] = [.screenRecording: .unknown, .accessibility: .unknown]
        for field in line.split(whereSeparator: \.isWhitespace) {
            let parts = field.split(separator: "=", maxSplits: 1).map(String.init)
            guard parts.count == 2 else { continue }
            let pane: PermissionFlowPane
            switch parts[0] {
            case "screen-recording": pane = .screenRecording
            case "accessibility": pane = .accessibility
            default: continue
            }
            states[pane] = parts[1] == "1" ? .on : PaneState(rawValue: parts[1]) ?? .unknown
        }
        return states
    }

    /// The answer `preflight-permissions` gives (run by main.swift).
    public nonisolated static func preflightReport() -> String {
        let screen = paneState(granted: PermissionRequester.screenRecordingGranted(), tcc: tccPreflight(.screenRecording))
        let ax = paneState(granted: PermissionRequester.accessibilityGranted(), tcc: tccPreflight(.accessibility))
        return "screen-recording=\(screen.rawValue) accessibility=\(ax.rawValue)"
    }

    /// What follows a grant.
    enum Next: Equatable {
        /// The other permission is still missing: guide it next.
        case guide(PermissionFlowPane)
        /// Nothing left: back to Cua Spaces.
        case finish
    }

    /// The two guided permissions, in the order the permissions screen lists
    /// them.
    nonisolated static let guided: [PermissionFlowPane] = [.screenRecording, .accessibility]

    /// What comes after `granted`: the other guided permission when it is
    /// still missing, else the end.
    nonisolated static func next(after granted: PermissionFlowPane, isGranted: (PermissionFlowPane) -> Bool) -> Next {
        guided.first { $0 != granted && !isGranted($0) }.map(Next.guide) ?? .finish
    }

    /// macOS applies a Screen Recording grant to a running app only after it
    /// reopens (System Settings offers "Quit & Reopen"); Accessibility
    /// applies at once.
    nonisolated static func needsReopen(after granted: PermissionFlowPane) -> Bool { granted == .screenRecording }

    private static var watching: Task<Void, Never>?
    /// Checked every second, as Cua Driver's guide does: the panel's words
    /// change as soon as the app shows up in the list or its switch turns on.
    nonisolated static let pollInterval: Duration = .seconds(1)
    nonisolated static let giveUpAfter: Duration = .seconds(15 * 60)

    private static var reopening: Task<Void, Never>?

    private static func watch(_ pane: PermissionFlowPane) {
        watching?.cancel()
        reopening?.cancel()
        watching = Task { @MainActor in
            let clock = ContinuousClock()
            let deadline = clock.now + giveUpAfter
            while !Task.isCancelled, clock.now < deadline {
                let now = await readStates()
                guard !Task.isCancelled else { return }
                lastStates = now
                let state = now[pane] ?? .unknown
                controller?.setGuidance(guidance(for: pane, state: state, appName: appName(), paneName: paneName(pane)))
                // Dock again under System Settings as it is now: after a switch
                // out of a full-screen Cua Spaces, the screen's free area is
                // only right once System Settings' Space is in front.
                controller?.refit()
                if state == .on { granted(pane, others: granted(in: now)); return }
                try? await Task.sleep(for: pollInterval)
            }
            // Gave up (a replaced or finished watch must not close a newer panel).
            if !Task.isCancelled {
                controller?.closePanel()
                clearGuiding()
            }
        }
    }

    private static func granted(_ pane: PermissionFlowPane, others: Set<PermissionFlowPane>) {
        watching = nil
        reopening?.cancel()
        if let hook = grantedHook { return hook(pane) }
        let followUp = {
            controller?.closePanel()
            switch next(after: pane, isGranted: others.contains) {
            case .guide(let other):
                guard let url = settingsURL(for: other) else { return }
                guide(url)
                // System Settings ignores a pane URL while macOS's own
                // "Quit & Reopen" sheet is still up: ask again once it is gone.
                if needsReopen(after: pane), let u = URL(string: url) {
                    reopening = Task { @MainActor in
                        for delay in [1, 3, 6] {
                            try? await Task.sleep(for: .seconds(delay))
                            guard !Task.isCancelled else { return }
                            NSWorkspace.shared.open(u)
                        }
                    }
                }
            case .finish:
                clearGuiding()
                bringAppForward()
            }
        }
        // The panel says "on" for a moment before it moves on or closes.
        guard needsReopen(after: pane) else {
            Task { @MainActor in
                try? await Task.sleep(for: .milliseconds(1200))
                followUp()
            }
            return
        }
        // Give macOS's own "Quit & Reopen" a moment first: when it is
        // answered, this app is already gone, and comes back through
        // `resumeAfterRelaunch`.
        Task { @MainActor in
            try? await Task.sleep(for: .seconds(1))
            bringAppForward()
            let alert = NSAlert()
            alert.messageText = "Cua Spaces needs to reopen to finish"
            alert.informativeText = "macOS applies Screen Recording after the app reopens. Your place is kept."
            alert.addButton(withTitle: "Reopen Cua Spaces")
            alert.addButton(withTitle: "Later")
            if alert.runModal() == .alertFirstButtonReturn {
                SparkleUpdater.relaunch(Bundle.main.bundleURL)
            } else {
                followUp()
            }
        }
    }

    /// Cua Spaces to the front, over System Settings. NSApp.activate() alone
    /// is only a request that macOS 14+ declines while another app is in
    /// front, so the app also asks Launch Services to open it, which brings
    /// it forward the way a click on its Dock icon does.
    static func bringAppForward() {
        NSApp.activate()
        let config = NSWorkspace.OpenConfiguration()
        config.activates = true
        NSWorkspace.shared.openApplication(at: Bundle.main.bundleURL, configuration: config)
        NSApp.windows.first { $0.isVisible && $0.canBecomeMain }?.makeKeyAndOrderFront(nil)
    }

    /// The name System Settings lists this app under: "Cua Spaces", or
    /// "Cua Spaces Local" for the Local build, which has its own bundle id
    /// and so its own grants.
    nonisolated static func appName(info: [String: Any]? = Bundle.main.infoDictionary) -> String {
        for key in ["CFBundleDisplayName", "CFBundleName"] {
            if let name = info?[key] as? String, !name.isEmpty { return name }
        }
        return "Cua Spaces"
    }

    /// The two guided permissions as the pages read them (`AppPermission`
    /// in the web's `ops/driver.ts`): `screen` and `accessibility`, each
    /// granted or not, with its System Settings pane.
    /// `state` is where the switch stands (`on`, `off`, `absent`,
    /// `unknown`), for the row's live status.
    nonisolated static func rows(states: [PermissionFlowPane: PaneState]) -> [[String: Any]] {
        let panes: [(String, PermissionFlowPane)] = [("screen", .screenRecording), ("accessibility", .accessibility)]
        return panes.map { id, pane -> [String: Any] in
            let url: Any = settingsURL(for: pane).map { $0 as Any } ?? NSNull()
            let state = states[pane] ?? .unknown
            return ["id": id, "granted": state == .on, "state": state.rawValue, "settingsUrl": url]
        }
    }

    nonisolated static func settingsURL(for pane: PermissionFlowPane) -> String? {
        switch pane {
        case .screenRecording: "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
        case .accessibility: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        default: nil
        }
    }
}
