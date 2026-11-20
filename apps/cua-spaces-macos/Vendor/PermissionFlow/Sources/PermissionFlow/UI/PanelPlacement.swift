#if os(macOS)
import CoreGraphics

/// Cua Spaces addition (CUA-3551): where the guide panel docks under System
/// Settings, as plain geometry so it can be tested without a screen.
///
/// The panel sits under the System Settings window's content area and is
/// kept inside the part of the screen the Dock and the menu bar leave free.
/// That part comes from `NSScreen.visibleFrame`, which describes the Space
/// of the app that asks. When Cua Spaces is full screen, its Space has no
/// Dock (and often no menu bar), so `visibleFrame` is the whole screen,
/// while System Settings opens on a desktop Space that has both. Clamping to
/// that frame put the panel's drag row behind the Dock (Mac test, 7 Nov).
/// `usableArea` therefore trusts `visibleFrame` only when it really shows
/// the Dock and this app isn't full screen; otherwise the panel stays above
/// the System Settings window's own bottom edge, which macOS keeps clear of
/// the Dock, and under the menu bar.
public struct PanelScreen: Equatable, Sendable {
    public var frame: CGRect
    public var visibleFrame: CGRect

    public init(frame: CGRect, visibleFrame: CGRect) {
        self.frame = frame
        self.visibleFrame = visibleFrame
    }
}

public enum PanelPlacement {
    /// System Settings' leading sidebar: the panel lines up with the pane
    /// to its right, where the person works.
    public static let sidebarWidth: CGFloat = 230
    /// The gap kept from the edges of the usable area.
    public static let screenInset: CGFloat = 12

    /// The screen System Settings is mostly on.
    public static func screen(for settingsFrame: CGRect, among screens: [PanelScreen]) -> PanelScreen? {
        func overlap(_ s: PanelScreen) -> CGFloat {
            let i = s.frame.intersection(settingsFrame)
            return i.isNull ? 0 : i.width * i.height
        }
        return screens.filter { overlap($0) > 0 }.max { overlap($0) < overlap($1) }
    }

    /// The part of `screen` the panel may cover while System Settings is at
    /// `settingsFrame`. `hostIsFullScreen`: a window of this app is full
    /// screen, so `visibleFrame` describes that Space, not System Settings'.
    /// `menuBarHeight`: the desktop's menu bar (with a notch, the taller of
    /// the two).
    public static func usableArea(screen: PanelScreen, settingsFrame: CGRect,
                                  hostIsFullScreen: Bool, menuBarHeight: CGFloat) -> CGRect {
        let frame = screen.frame
        let visible = screen.visibleFrame
        let showsDock = visible.minY > frame.minY + 0.5 || visible.minX > frame.minX + 0.5 || visible.maxX < frame.maxX - 0.5
        if showsDock && !hostIsFullScreen { return visible }
        // The Dock's place is unknown here: keep above System Settings' own
        // bottom edge, and under the menu bar.
        let top = min(visible.maxY, frame.maxY - menuBarHeight)
        let bottom = max(frame.minY, min(settingsFrame.minY, top))
        return CGRect(x: frame.minX, y: bottom, width: frame.width, height: max(0, top - bottom))
    }

    /// The panel's frame: under System Settings' content area, as wide as
    /// that area, `height(width)` tall, moved (never resized) to stay
    /// `screenInset` inside `usable`. With no room under the window it
    /// overlaps the window's bottom instead of leaving the screen.
    public static func dockedFrame(settingsFrame: CGRect, usable: CGRect,
                                   height: (CGFloat) -> CGFloat) -> CGRect {
        let contentWidth = max(240, settingsFrame.width - sidebarWidth)
        let width = min(contentWidth, usable.width - (screenInset * 2))
        let h = height(width)
        var origin = CGPoint(x: settingsFrame.minX + sidebarWidth, y: settingsFrame.minY - h)
        origin.x = max(usable.minX + screenInset, min(origin.x, usable.maxX - width - screenInset))
        // The bottom wins over the top: the drag row is at the panel's bottom.
        origin.y = max(usable.minY + screenInset, min(origin.y, usable.maxY - h - screenInset))
        return CGRect(origin: origin, size: CGSize(width: width, height: h))
    }
}
#endif
