// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
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
        return true
    }
}
