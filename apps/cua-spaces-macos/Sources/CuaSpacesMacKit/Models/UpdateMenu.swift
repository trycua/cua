// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation

/// The update entry of the app menu and the menu bar menu, and whether the
/// menu bar icon carries the up-arrow badge. One value from the one model
/// (`UpdatesModel.menuItem`), so both menus and Settings agree.
public struct UpdateMenuItem: Equatable, Sendable {
    public enum Action: Equatable, Sendable {
        /// Check for updates now.
        case check
        /// Install the found update: restart into it, or as soon as it is downloaded.
        case install
        /// Nothing to do (a check or a download is running).
        case none
    }

    /// The item's first line.
    public var title: String
    /// The second line, when there is one.
    public var detail: String?
    public var action: Action
    /// The menu bar icon's up-arrow: an update was found and is not installed.
    public var badge: Bool

    public var enabled: Bool { action != .none }

    /// The entry in `Localizable.xcstrings`; the English here is what a build
    /// that did not compile the catalog (`swift test`) shows.
    private static func text(_ code: String, _ english: String, vars: [String: String] = [:]) -> String {
        var out = (ModuleResources.bundle ?? .main).localizedString(forKey: code, value: english, table: nil)
        for (name, value) in vars { out = out.replacingOccurrences(of: "{\(name)}", with: value) }
        return out
    }

    public static func make(_ state: UpdateState) -> UpdateMenuItem {
        let check = Self.text("menu.update.check", "Check for Updates\u{2026}")
        let found = state.version.map { Self.text("menu.update.available", "Version {version} available", vars: ["version": $0]) } ?? check
        switch state.status {
        case .idle:
            return UpdateMenuItem(title: check, action: .check, badge: false)
        case .checking:
            return UpdateMenuItem(title: Self.text("menu.update.checking", "Checking for Updates\u{2026}"), action: .none, badge: false)
        case .available:
            return UpdateMenuItem(title: found, detail: Self.text("menu.update.click_to_install", "Click to install"),
                                  action: .install, badge: true)
        case .downloading:
            return UpdateMenuItem(
                title: found,
                detail: Self.text("menu.update.downloading", "Downloading {percent}%", vars: ["percent": String(state.percent ?? 0)]),
                action: .none, badge: true)
        case .downloaded:
            return UpdateMenuItem(title: found, detail: Self.text("menu.update.restart", "Restart to update"),
                                  action: .install, badge: true)
        case .error:
            // A failed check is a plain line; the item still checks again.
            return UpdateMenuItem(title: check, detail: Self.text("menu.update.failed", "Could not check for updates"),
                                  action: .check, badge: false)
        }
    }
}
