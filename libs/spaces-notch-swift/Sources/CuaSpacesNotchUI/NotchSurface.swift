// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import SwiftUI

/// What the notch views read and where their input goes. The SwiftUI app's
/// `NotchController` implements it over the app core; the helper's
/// `HelperNotch` over the notch protocol. Implementations are `@Observable`,
/// so the views follow every property they read.
@MainActor
public protocol NotchSurface: AnyObject {
    /// The core's view.
    var view: NotchData.View { get }
    /// The header's search text.
    var query: String { get }
    /// Sizes for the notch screen.
    var geometry: NotchGeometry { get }
    /// The core's motion.
    var motion: NotchData.Motion { get }
    /// The closed and open radii.
    var radii: NotchData.RadiiPair { get }
    /// A forced hover or pressed look (snapshot tests, debug starts).
    var highlight: NotchHighlight? { get }
    /// The dragged window's image (the additive ghost).
    var ghost: NSImage? { get }
    /// A Space's latest thumbnail.
    func thumbnail(_ spaceId: String) -> NSImage?
    /// An OS icon id's symbol or SVG (the core's `appOsIconSystemSymbol`,
    /// `appOsIconSvg`).
    func osIcon(_ id: String) -> NotchData.OsIcon?
    /// The core's estimated progress for a ring without real progress
    /// (`appNotchEstimatedProgress`).
    func estimatedProgress(elapsedMs: Int64, estimateMs: UInt32) -> UInt32

    /// Hover, clicks, Escape, a file drag over the notch.
    func send(_ event: NotchData.Event)
    /// The header's search text changed.
    func search(_ query: String)
    /// A tile was clicked.
    func openSpace(_ spaceId: String)
    /// A header button was clicked.
    func run(_ button: NotchData.ButtonId)
    /// The live-access line was clicked: the Keyvault's Access page.
    func openAccess()
    /// Its Dismiss: hides the indicator and the tiles' key.
    func dismissAccess()
    /// The permission line's button.
    func openPermissionSettings(pane: String)
    /// Apps or files were dropped on a tile.
    func drop(_ urls: [URL], on spaceId: String)
    /// The view finished stepping to a new stage.
    func stageChanged(open: Bool)
    /// The search field takes the keyboard.
    func takeKeyboard()
    /// The scene's window actions, from the view's environment (the SwiftUI
    /// app opens its main window and Settings with them).
    func attach(openMain: @escaping () -> Void, openSettings: @escaping () -> Void)
}

private struct NotchOsIconKey: EnvironmentKey {
    static let defaultValue: (String) -> NotchData.OsIcon? = { _ in nil }
}

private struct NotchEstimateKey: EnvironmentKey {
    static let defaultValue: (Int64, UInt32) -> UInt32 = { _, _ in 0 }
}

extension EnvironmentValues {
    /// Looks up an OS icon id for the tiles (`NotchSurface.osIcon`; set by
    /// `NotchContentView`, or by a host that draws a tile on its own).
    public var notchOsIcon: (String) -> NotchData.OsIcon? {
        get { self[NotchOsIconKey.self] }
        set { self[NotchOsIconKey.self] = newValue }
    }

    /// The estimated progress in thousandths for an elapsed time and an
    /// estimate (`NotchSurface.estimatedProgress`).
    public var notchEstimate: (Int64, UInt32) -> UInt32 {
        get { self[NotchEstimateKey.self] }
        set { self[NotchEstimateKey.self] = newValue }
    }
}
