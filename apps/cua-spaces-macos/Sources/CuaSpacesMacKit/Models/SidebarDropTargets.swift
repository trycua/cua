// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
import Observation
import SwiftUI

/// Space rows in the main window's sidebar as drop targets for a dragged
/// real window (the notch's tiles are the other target). The drag itself is
/// the SDK's window-drag monitor, which the notch controller already
/// follows; this hit-tests the global cursor against the rows. Which Spaces
/// take a drop is the core's decision (`appSpaceAcceptsDrop`, the notch
/// tiles' rule too).
@MainActor
@Observable
public final class SidebarDropTargets {
    /// The row under a dragged window.
    public private(set) var targetedId: String?
    /// Row frames in the window's content, top-left origin (SwiftUI's
    /// window space).
    @ObservationIgnored private(set) var frames: [String: CGRect] = [:]
    @ObservationIgnored weak var window: NSWindow?
    /// Whether a Space takes a drop (the core's rule).
    @ObservationIgnored var isDropTarget: (String) -> Bool = { _ in false }
    /// The window at a screen point below the dragged window, or nil when
    /// that cannot be told (the dragged window itself is under the cursor).
    @ObservationIgnored var windowUnder: (NSPoint, UInt32?) -> Int? = { point, dragged in
        NSWindow.windowNumber(at: point, belowWindowWithWindowNumber: dragged.map(Int.init) ?? 0)
    }

    /// Whether the window is on screen (tests never order one front).
    @ObservationIgnored var visible: (NSWindow) -> Bool = { $0.isVisible && !$0.isMiniaturized }

    public init() {}

    func setFrame(_ id: String, _ rect: CGRect) { frames[id] = rect }
    func removeFrame(_ id: String) { frames[id] = nil }

    /// A global top-left point (the drag events' space: origin at the
    /// primary display's top left, y down) in a window's content, top-left
    /// origin. `windowFrame` is AppKit's (bottom-left origin).
    nonisolated static func contentPoint(global: CGPoint, primaryTop: CGFloat, windowFrame: CGRect,
                                         contentHeight: CGFloat) -> CGPoint {
        let screen = CGPoint(x: global.x, y: primaryTop - global.y)
        return CGPoint(x: screen.x - windowFrame.minX, y: contentHeight - (screen.y - windowFrame.minY))
    }

    /// The drop-target row containing a content point.
    func row(atContent point: CGPoint) -> String? {
        frames.first { $0.value.contains(point) && isDropTarget($0.key) }?.key
    }

    /// The drop-target row under a global point, when the main window is
    /// visible there and not covered by another window.
    func row(atGlobal global: CGPoint, dragged: UInt32?) -> String? {
        guard let window, visible(window), let content = window.contentView else { return nil }
        let primaryTop = NSScreen.screens.first?.frame.maxY ?? 0
        let screen = NSPoint(x: global.x, y: primaryTop - global.y)
        guard window.frame.contains(screen) else { return nil }
        if let under = windowUnder(screen, dragged), under != window.windowNumber { return nil }
        return row(atContent: Self.contentPoint(global: global, primaryTop: primaryTop,
                                                windowFrame: window.frame,
                                                contentHeight: content.frame.height))
    }

    /// Follows one drag event. Returns the Space a release over a row
    /// commits to.
    @discardableResult
    func handle(phase: String, target: String?) -> String? {
        switch phase {
        case "start", "move":
            targetedId = target
            return nil
        case "end":
            let committed = target
            targetedId = nil
            return committed
        default:
            targetedId = nil
            return nil
        }
    }
}

/// Hands the hosting window to a closure (the drop targets need it to map
/// the global cursor into the sidebar).
struct WindowReader: NSViewRepresentable {
    let onWindow: (NSWindow?) -> Void

    func makeNSView(context: Context) -> NSView {
        let view = ReaderView()
        view.onWindow = onWindow
        return view
    }

    func updateNSView(_ nsView: NSView, context: Context) {}

    final class ReaderView: NSView {
        var onWindow: ((NSWindow?) -> Void)?
        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            onWindow?(window)
        }
    }
}
