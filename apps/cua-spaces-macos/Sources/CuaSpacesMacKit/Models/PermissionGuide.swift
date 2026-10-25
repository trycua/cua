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

    /// Guides `url`'s pane: false when the guide doesn't know it (open it
    /// plainly then).
    @discardableResult
    public static func guide(_ url: String) -> Bool {
        guard let pane = pane(for: url) else { return false }
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
    /// System Settings until closed. So the guide watches the same preflight
    /// calls the permission rows do (CGPreflightScreenCaptureAccess,
    /// AXIsProcessTrusted) and closes the panel the moment its pane is
    /// granted. Replaceable in tests.
    nonisolated(unsafe) static var isGranted: (PermissionFlowPane) -> Bool = { pane in
        switch pane {
        case .screenRecording: CGPreflightScreenCaptureAccess()
        case .accessibility: AXIsProcessTrusted()
        default: false
        }
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

    private static func watch(_ pane: PermissionFlowPane) {
        watching?.cancel()
        watching = Task { @MainActor in
            let clock = ContinuousClock()
            let deadline = clock.now + giveUpAfter
            while !Task.isCancelled, clock.now < deadline {
                if isGranted(pane) { granted(pane); return }
                try? await Task.sleep(for: pollInterval)
            }
        }
    }

    private static func granted(_ pane: PermissionFlowPane) {
        watching = nil
        controller?.closePanel()
        let followUp = {
            switch next(after: pane, isGranted: isGranted) {
            case .guide(let other):
                if let url = settingsURL(for: other) { guide(url) }
            case .finish:
                NSApp.activate()
            }
        }
        guard needsReopen(after: pane) else { return followUp() }
        // Give macOS's own "Quit & Reopen" a moment first: when it is
        // answered, this app is already gone.
        Task { @MainActor in
            try? await Task.sleep(for: .seconds(1))
            NSApp.activate()
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

    nonisolated static func settingsURL(for pane: PermissionFlowPane) -> String? {
        switch pane {
        case .screenRecording: "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
        case .accessibility: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        default: nil
        }
    }
}
