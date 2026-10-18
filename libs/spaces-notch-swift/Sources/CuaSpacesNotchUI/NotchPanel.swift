// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import SwiftUI

/// The notch overlay: a borderless, non-activating panel above the menu bar
/// that joins every Space and full-screen app. It covers the core's stage
/// frame (every notch state plus shadow room) and never resizes while the
/// shape animates; its transparent pixels let clicks through, so only the
/// drawn notch (and a nearly transparent hover margin) takes events.
///
/// Level, collection behaviour and the constrained frame are derived from
/// CodeIsland and NotchDrop (MIT); see THIRD_PARTY_NOTICES.md. Geometry
/// comes from `safeAreaInsets` and the auxiliary top areas.
public final class NotchPanel: NSPanel {
    public init() {
        super.init(contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel],
                   backing: .buffered, defer: false)
        isFloatingPanel = true
        hidesOnDeactivate = false
        becomesKeyOnlyIfNeeded = true
        isOpaque = false
        backgroundColor = .clear
        hasShadow = false
        isMovable = false
        animationBehavior = .none
        isReleasedWhenClosed = false
        level = NSWindow.Level(rawValue: NSWindow.Level.mainMenu.rawValue + 3)
        collectionBehavior = [.canJoinAllSpaces, .stationary, .fullScreenAuxiliary, .ignoresCycle]
        sharingType = .readOnly
        title = "Cua Spaces notch"
    }

    override public var canBecomeKey: Bool { true }
    override public var canBecomeMain: Bool { false }

    /// Stay on top of the menu bar after wake or a display change.
    override public func constrainFrameRect(_ frameRect: NSRect, to screen: NSScreen?) -> NSRect { frameRect }
}

/// Takes the first click on a non-activating panel.
public final class NotchHostingView<Content: View>: NSHostingView<Content> {
    override public func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
}

/// The screens the notch reads.
@MainActor
public enum NotchScreens {
    /// The screen: the built-in notched display when there is one, else the
    /// main one (else the first: an agent app without a key window may have
    /// no main screen).
    public static func screen() -> NSScreen? {
        NSScreen.screens.first { $0.safeAreaInsets.top > 0 } ?? NSScreen.main ?? NSScreen.screens.first
    }

    /// The primary display (whose top the drag events measure from).
    public static func primary() -> NSScreen? { NSScreen.screens.first }

    /// A screen's facts for the core's layout.
    public static func facts(_ s: NSScreen) -> NotchData.ScreenFacts {
        func rect(_ r: NSRect) -> NotchData.Rect {
            NotchData.Rect(x: r.origin.x, y: r.origin.y, width: r.width, height: r.height)
        }
        return NotchData.ScreenFacts(frame: rect(s.frame), visibleFrame: rect(s.visibleFrame),
                                     safeAreaTop: s.safeAreaInsets.top,
                                     auxLeftWidth: s.auxiliaryTopLeftArea.map { Double($0.width) },
                                     auxRightWidth: s.auxiliaryTopRightArea.map { Double($0.width) })
    }

    /// Whether the open panel shows a line above the tiles (a drop hint, the
    /// permission line or live access): the layout then has that row.
    public static func needsRow(_ v: NotchData.View) -> Bool { v.prompt != nil || v.permission != nil || v.access != nil }
}
