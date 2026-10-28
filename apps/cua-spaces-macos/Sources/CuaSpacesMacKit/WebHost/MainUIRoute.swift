// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import Foundation

/// Where the app's chrome (the notch, the menu bar item) opens the main UI:
/// a page of the web UI window, at the routes the Electron app opens for the
/// same actions (apps/cua-spaces-desktop/src/main.ts).
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
    /// A page path as given (`/machines?join=<CODE>` from a `cua://add` link).
    case path(String)

    /// The first run, asked for by a person (Settings, Welcome): the shared
    /// web onboarding, always shown.
    static let onboarding = MainUIRoute.path("/onboarding")

    /// The first run, opened because it is not done (a launch, or the window
    /// opening again): the same page, which goes to Spaces instead when the
    /// person chose "Skip for now", so the welcome does not come back at every
    /// launch. The page keeps that choice, `due` asks it to honour it.
    static let firstRunDue = MainUIRoute.path("/onboarding?due=1")

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
        case let .path(path): return path
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

/// The one place the app's chrome (the Dock icon, the notch, the menu bar
/// item, the menus, ⌘⇧Space, notifications) opens the main UI: the web UI
/// window (`WebUIWindowController`) at a route. It is the app's only main
/// window; there is no native one to fall back to.
@MainActor
struct MainUIRouter {
    let model: AppModel
    /// Opens the web UI window at a route (tests set their own).
    var web: (MainUIRoute) -> Void
    /// Opens the page's Settings (tests set their own).
    var settings: (MainUIRoute) -> Void

    init(model: AppModel, web: ((MainUIRoute) -> Void)? = nil, settings: ((MainUIRoute) -> Void)? = nil) {
        self.model = model
        self.web = web ?? { WebUIWindowController.show(model: model, at: $0) }
        self.settings = settings ?? { WebUIWindowController.openSettings(model: model, at: $0) }
    }

    func open(_ route: MainUIRoute) {
        web(route)
    }

    /// The main window as the person left it, or the first run while it is
    /// due (`launchRoute`): the Dock icon, Open, ⌘⇧Space, the notch's list.
    func openMain() {
        if let route = Self.launchRoute(onboarded: model.onboarding.completed) {
            web(route)
        } else if WebUIWindowController.shared != nil {
            // Open already: in front, on the page it shows.
            WebUIWindowController.show(model: model)
        } else {
            web(.home)
        }
    }

    /// Where a window opens while the first run may be due: its page until it
    /// is done (`.firstRunDue`, which keeps a "Skip for now"), else nil (the
    /// page the window shows, or Spaces).
    static func launchRoute(onboarded: Bool) -> MainUIRoute? {
        onboarded ? nil : .firstRunDue
    }

    /// The page a click on one of the app's notifications opens: a Space that
    /// came up (its page), a machine asking to be added (Machines, where
    /// it waits with Add machine and Not me), an agent's answer or request (Agents), else Spaces.
    static func route(forNotification id: String) -> MainUIRoute {
        if let spaceId = SpaceReadyNote.spaceId(fromIdentifier: id) { return .space(spaceId) }
        if id.hasPrefix("device-") { return .path("/machines") }
        if id.hasPrefix("agent-") { return .agents }
        return .home
    }

    /// Settings (⌘,, the menu bar item, the notch, the palette): the page's
    /// Settings in the web UI window. There is no native Settings window,
    /// so the two can't drift. `route`: one tab
    /// (`/settings/connectors`, ...).
    func openSettings(_ route: MainUIRoute = .settings) {
        settings(route)
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
