// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit

/// `CUA_SPACES_OFFSCREEN=1` (debug builds only, through `DevHooks`): a run
/// nobody sees, for captures on a Mac someone is working at. The app is an
/// accessory (no Dock icon, no menu bar), the menu bar item and the notch are
/// never put up, the global shortcut is never taken, and nothing activates
/// the app. Its windows are made, ordered in and painted, so
/// `WKWebView.takeSnapshot` works, but are fully transparent, click-through
/// and shadowless, and never become key.
public enum OffscreenRun {
    public static var isOn: Bool { DevHooks.value("CUA_SPACES_OFFSCREEN") == "1" }

    /// Before anything shows (`main.swift`, ahead of the app's scenes).
    public static func prepare() {
        guard isOn else { return }
        MainActor.assumeIsolated { _ = NSApplication.shared.setActivationPolicy(.accessory) }
    }

    /// Makes `window` invisible and untouchable; it still paints.
    @MainActor static func hide(_ window: NSWindow) {
        guard isOn else { return }
        window.alphaValue = 0
        window.ignoresMouseEvents = true
        window.hasShadow = false
        window.collectionBehavior.formUnion([.transient, .ignoresCycle, .stationary])
    }
}
