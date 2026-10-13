// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import Foundation

/// Where the app's chrome (the notch, the menu bar item) opens the main UI.
/// With New UI on it is a page of the New UI window, at the routes the
/// Electron app opens for the same actions (apps/cua-spaces-desktop/src/main.ts).
public enum MainUIRoute: Equatable, Sendable {
    /// The Spaces list.
    case home
    /// A Space's page; `dropped` are the apps and files dropped on its notch
    /// tile, which the page handles as a drop on its well (an app opens
    /// Teleport at it, files are sent).
    case space(String, dropped: [URL] = [])
    case settings
    /// The Keyvault's Access page.
    case access
    /// Cua Volume.
    case drive
    case machines
    case agents
    case keyvault

    /// The page's path, query included.
    var webPath: String {
        switch self {
        case .home: return "/spaces"
        case .settings: return "/settings"
        case .access: return "/keyvault?view=access"
        case .drive: return "/volume"
        case .machines: return "/machines"
        case .agents: return "/agents"
        case .keyvault: return "/keyvault"
        case let .space(id, dropped):
            let path = "/spaces/\(Self.encodeComponent(id))"
            let names = dropped.map(\.lastPathComponent)
            guard !names.isEmpty, let data = try? JSONEncoder().encode(names),
                  let json = String(data: data, encoding: .utf8) else { return path }
            return "\(path)?dropped=\(Self.encodeComponent(json))"
        }
    }

    var dropped: [URL] {
        if case let .space(_, dropped) = self { return dropped }
        return []
    }

    /// JavaScript's `encodeURIComponent`.
    static func encodeComponent(_ s: String) -> String {
        var allowed = CharacterSet(charactersIn: "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789")
        allowed.insert(charactersIn: "-_.!~*'()")
        return s.addingPercentEncoding(withAllowedCharacters: allowed) ?? s
    }
}

/// The one place the app's chrome (the notch, the menu bar item, the menus,
/// notifications) opens the main UI: the New UI window at a route while its
/// experiment is on, else the native window or Settings as before.
@MainActor
struct MainUIRouter {
    let model: AppModel
    /// Opens the New UI window at a route (tests set their own).
    var web: (MainUIRoute) -> Void

    init(model: AppModel, web: ((MainUIRoute) -> Void)? = nil) {
        self.model = model
        self.web = web ?? { WebUIWindowController.show(model: model, at: $0) }
    }

    func open(_ route: MainUIRoute, native: () -> Void) {
        if model.settings.experiments.webUi { web(route) } else { native() }
    }

    /// The page a View menu `nav.*` command names.
    static func route(forNav command: String) -> MainUIRoute? {
        switch command {
        case "nav.spaces": return .home
        case "nav.machines": return .machines
        case "nav.agents": return .agents
        case "nav.keyvault": return .keyvault
        case "nav.settings": return .settings
        default: return nil
        }
    }
}

/// New UI (preview) is the main window while it is on, not a second one:
/// turning it on puts its window where the native one was (same frame) and
/// closes that; turning it off does the reverse. Whatever opens the native
/// main window while it is on (the Dock, the Window menu, a path not routed
/// through `MainUIRouter`) gets the New UI window in its place.
@MainActor
enum MainWindowSwitch {
    /// The native main window (`Window(id: "main")`), while it exists.
    static weak var nativeWindow: NSWindow?
    /// Where the native window opens next: New UI's frame when it was turned off.
    static var pendingNativeFrame: NSRect?

    /// Whether the native window showing the main UI (not the launch or the
    /// first run) gives way to New UI.
    static func replacesNative(webUi: Bool, ready: Bool, onboarded: Bool) -> Bool {
        webUi && ready && onboarded
    }

    static func replacesNative(_ model: AppModel) -> Bool {
        replacesNative(webUi: model.settings.experiments.webUi, ready: model.startup.isReady,
                       onboarded: model.onboarding.completed)
    }

    /// The native window is showing the main UI (`MainWindow` appeared in
    /// it): with New UI on, New UI opens in its frame and it closes.
    static func nativeAppeared(_ window: NSWindow?, model: AppModel,
                               web: ((NSRect?) -> Void)? = nil) {
        if let window { nativeWindow = window }
        guard let window = window ?? nativeWindow else { return }
        if let frame = pendingNativeFrame {
            pendingNativeFrame = nil
            // After SwiftUI placed it.
            DispatchQueue.main.async { window.setFrame(frame, display: true) }
        }
        guard replacesNative(model) else { return }
        replace(window, model: model, web: web)
    }

    /// New UI was turned on: its window takes the native one's place.
    static func switchToWebUI(model: AppModel, web: ((NSRect?) -> Void)? = nil) {
        guard let window = nativeWindow, window.isVisible else {
            // No native window up (only Settings, say): New UI opens as at launch.
            if model.startup.isReady, model.onboarding.completed { (web ?? { show(model, frame: $0) })(nil) }
            return
        }
        replace(window, model: model, web: web)
    }

    /// The native window being closed for New UI (its reader and its
    /// `onAppear` both report it).
    private static weak var replacing: NSWindow?

    private static func replace(_ window: NSWindow, model: AppModel, web: ((NSRect?) -> Void)?) {
        guard replacing !== window else { return }
        replacing = window
        let frame = window.frame.width > 0 ? window.frame : nil
        // Never seen: hidden before it draws, where it can.
        window.alphaValue = 0
        (web ?? { show(model, frame: $0) })(frame)
        DispatchQueue.main.async {
            window.close()
            window.alphaValue = 1
            if replacing === window { replacing = nil }
        }
    }

    private static func show(_ model: AppModel, frame: NSRect?) {
        WebUIWindowController.show(model: model, frame: frame)
    }

    /// New UI was turned off: the native window opens in its frame.
    static func switchToNative(webFrame: NSRect?, openMain: (() -> Void)?) {
        pendingNativeFrame = webFrame
        openMain?()
    }
}
