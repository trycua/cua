// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

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

    /// The page's path, query included.
    var webPath: String {
        switch self {
        case .home: return "/spaces"
        case .settings: return "/settings"
        case .access: return "/keyvault?view=access"
        case .drive: return "/volume"
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

/// The one place the notch and the menu bar item open the main UI: the New
/// UI window at a route while its experiment is on, else the native window
/// or Settings as before.
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
}
