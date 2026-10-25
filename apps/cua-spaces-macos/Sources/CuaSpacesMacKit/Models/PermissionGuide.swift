// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import ApplicationServices
import CoreGraphics
import PermissionFlow

/// The app's own guide to a privacy pane of System Settings, so nobody
/// meets macOS's bare "allow?" prompt first. PermissionFlow (MIT,
/// github.com/jaywcjlove/PermissionFlow, pinned in Package.swift) opens the
/// pane and floats a small panel beside System Settings with this app to
/// drag into the list. The app's name and icon are this bundle's, so Cua
/// Spaces Local shows as itself.
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

    // MARK: - Listed or not (CUA-2283)

    /// What the guide tells the person to do in a pane's list.
    enum Instruction: Equatable {
        /// The app is not in the list yet: drag it in (PermissionFlow's panel).
        case drag
        /// The app is already in the list: only its switch needs turning on.
        case switchOn
    }

    /// macOS has no public call that says whether an app has a row in a
    /// privacy list, so the guide remembers it: the permission was seen
    /// granted (a revoked grant keeps its row). An app dragged in but never
    /// switched on is not noticed, and keeps the drag guide. An
    /// uninstall that resets the permissions removes the app's preferences
    /// with this note.
    nonisolated(unsafe) static var listedStore: UserDefaults = .standard

    nonisolated static func listedKey(_ pane: PermissionFlowPane) -> String { "permissionListed.\(pane.rawValue)" }

    nonisolated static func isListed(_ pane: PermissionFlowPane) -> Bool {
        listedStore.bool(forKey: listedKey(pane))
    }

    nonisolated static func markListed(_ pane: PermissionFlowPane, _ listed: Bool = true) {
        listedStore.set(listed, forKey: listedKey(pane))
    }

    nonisolated static func instruction(for pane: PermissionFlowPane, listed: Bool) -> Instruction {
        listed ? .switchOn : .drag
    }

    /// The line of the switch-on hint.
    nonisolated static func switchOnText(appName: String) -> String {
        "Switch on \(appName) in the list."
    }

    private static var hint: NSPanel?

    private static func closeHint() {
        hint?.close()
        hint = nil
    }

    /// A small panel over System Settings for a pane whose list already has
    /// this app: no dragging, just the switch. "Not in the list?" falls back
    /// to the drag guide.
    private static func showSwitchOnHint(for pane: PermissionFlowPane, url: String) {
        closeHint()
        let name = FileManager.default.displayName(atPath: Bundle.main.bundlePath)
        let label = NSTextField(labelWithString: switchOnText(appName: name))
        label.font = .systemFont(ofSize: 14)
        let fallback = NSButton(title: "Not in the list?", target: nil, action: nil)
        fallback.bezelStyle = .inline
        let close = NSButton(image: NSImage(systemSymbolName: "xmark.circle.fill", accessibilityDescription: "Close") ?? NSImage(),
                             target: nil, action: nil)
        close.isBordered = false
        let row = NSStackView(views: [label, fallback, close])
        row.spacing = 10
        row.edgeInsets = NSEdgeInsets(top: 10, left: 14, bottom: 10, right: 12)
        let box = NSVisualEffectView()
        box.material = .hudWindow
        box.state = .active
        box.wantsLayer = true
        box.layer?.cornerRadius = 12
        row.translatesAutoresizingMaskIntoConstraints = false
        box.addSubview(row)
        NSLayoutConstraint.activate([
            row.leadingAnchor.constraint(equalTo: box.leadingAnchor),
            row.trailingAnchor.constraint(equalTo: box.trailingAnchor),
            row.topAnchor.constraint(equalTo: box.topAnchor),
            row.bottomAnchor.constraint(equalTo: box.bottomAnchor),
        ])
        let panel = NSPanel(contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.level = .floating
        panel.hidesOnDeactivate = false
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        panel.isReleasedWhenClosed = false
        panel.contentView = box
        let size = box.fittingSize
        if let screen = NSScreen.main?.visibleFrame {
            panel.setFrame(NSRect(x: screen.midX - size.width / 2, y: screen.maxY - size.height - 16,
                                  width: size.width, height: size.height), display: true)
        }
        let actions = HintActions(
            close: { closeHint() },
            fallback: {
                markListed(pane, false)
                closeHint()
                guide(url)
            })
        fallback.target = actions
        fallback.action = #selector(HintActions.fallbackTapped)
        close.target = actions
        close.action = #selector(HintActions.closeTapped)
        objc_setAssociatedObject(panel, &HintActions.key, actions, .OBJC_ASSOCIATION_RETAIN)
        panel.orderFrontRegardless()
        hint = panel
    }

    private final class HintActions: NSObject {
        nonisolated(unsafe) static var key: UInt8 = 0
        let close: () -> Void
        let fallback: () -> Void
        init(close: @escaping () -> Void, fallback: @escaping () -> Void) {
            self.close = close
            self.fallback = fallback
        }
        @objc func closeTapped() { close() }
        @objc func fallbackTapped() { fallback() }
    }

    /// Guides `url`'s pane: false when the guide doesn't know it (open it
    /// plainly then).
    @discardableResult
    public static func guide(_ url: String) -> Bool {
        guard let pane = pane(for: url) else { return false }
        closeHint()
        // Already in the list: say "switch on", not "drag".
        if instruction(for: pane, listed: isListed(pane)) == .switchOn, let u = URL(string: url) {
            controller?.closePanel()
            NSWorkspace.shared.open(u)
            showSwitchOnHint(for: pane, url: url)
            watch(pane)
            return true
        }
        let app = Bundle.main.bundleURL
        let flow = controller ?? PermissionFlowController(configuration: .init(requiredAppURLs: [app]))
        controller = flow
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
    nonisolated(unsafe) static var readGranted: () async -> Set<PermissionFlowPane> = {
        await Task.detached { freshProcessGrants() }.value
    }

    /// `CuaSpacesMac preflight-permissions` in a new process: the guided
    /// panes macOS has granted this app.
    nonisolated static func freshProcessGrants() -> Set<PermissionFlowPane> {
        guard let exe = Bundle.main.executableURL else { return [] }
        let p = Process()
        p.executableURL = exe
        p.arguments = ["preflight-permissions"]
        let out = Pipe()
        p.standardOutput = out
        p.standardError = FileHandle.nullDevice
        guard (try? p.run()) != nil else { return [] }
        let data = out.fileHandleForReading.readDataToEndOfFile()
        p.waitUntilExit()
        return parseGrants(String(decoding: data, as: UTF8.self))
    }

    /// What `preflight-permissions` prints: `screen-recording=1 accessibility=0`.
    nonisolated static func parseGrants(_ line: String) -> Set<PermissionFlowPane> {
        var granted = Set<PermissionFlowPane>()
        for field in line.split(whereSeparator: \.isWhitespace) where field.hasSuffix("=1") {
            switch field.dropLast(2) {
            case "screen-recording": granted.insert(.screenRecording)
            case "accessibility": granted.insert(.accessibility)
            default: break
            }
        }
        return granted
    }

    /// The answer `preflight-permissions` gives (run by main.swift).
    public nonisolated static func preflightReport() -> String {
        "screen-recording=\(CGPreflightScreenCaptureAccess() ? 1 : 0) accessibility=\(AXIsProcessTrusted() ? 1 : 0)"
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
    nonisolated static let pollInterval: Duration = .milliseconds(500)
    nonisolated static let giveUpAfter: Duration = .seconds(15 * 60)

    private static var reopening: Task<Void, Never>?

    private static func watch(_ pane: PermissionFlowPane) {
        watching?.cancel()
        reopening?.cancel()
        watching = Task { @MainActor in
            let clock = ContinuousClock()
            let deadline = clock.now + giveUpAfter
            while !Task.isCancelled, clock.now < deadline {
                let now = await readGranted()
                guard !Task.isCancelled else { return }
                // A grant, even one revoked later, means the app has its row.
                for p in now { markListed(p) }
                if now.contains(pane) { granted(pane, others: now); return }
                try? await Task.sleep(for: pollInterval)
            }
            // Gave up (a replaced or finished watch must not close a newer hint).
            if !Task.isCancelled { closeHint() }
        }
    }

    private static func granted(_ pane: PermissionFlowPane, others: Set<PermissionFlowPane>) {
        watching = nil
        reopening?.cancel()
        controller?.closePanel()
        closeHint()
        let followUp = {
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
                bringAppForward()
            }
        }
        guard needsReopen(after: pane) else { return followUp() }
        // Give macOS's own "Quit & Reopen" a moment first: when it is
        // answered, this app is already gone.
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

    nonisolated static func settingsURL(for pane: PermissionFlowPane) -> String? {
        switch pane {
        case .screenRecording: "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
        case .accessibility: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        default: nil
        }
    }
}
