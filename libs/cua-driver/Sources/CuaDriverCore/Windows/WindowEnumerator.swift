import CoreGraphics
import Foundation

public enum WindowEnumerator {
    /// All on-screen windows, ordered back-to-front (lower ``zIndex`` = further back).
    /// Uses `CGWindowListCopyWindowInfo` — no TCC required, works for every app's
    /// windows even without Accessibility. Does not include hidden or minimized
    /// windows.
    public static func visibleWindows() -> [WindowInfo] {
        enumerate(options: [.optionOnScreenOnly, .excludeDesktopElements])
    }

    /// All windows known to WindowServer — including off-screen ones
    /// (hidden-launched, minimized into the Dock, on another Space).
    /// Each entry's `isOnScreen` field tells you whether it's visible
    /// right now. Use this when you want to identify a pid's top-level
    /// window regardless of current visibility (e.g. to capture a
    /// just-launched hidden window's backing store, or to find a
    /// minimized window's bounds to reference in window-local coords).
    public static func allWindows() -> [WindowInfo] {
        enumerate(options: [.excludeDesktopElements])
    }

    private static func enumerate(options: CGWindowListOption) -> [WindowInfo] {
        guard
            let raw = CGWindowListCopyWindowInfo(options, kCGNullWindowID)
                as? [[String: Any]]
        else {
            return []
        }
        // CGWindowList returns front-to-back; assign zIndex so that larger = frontmost.
        let total = raw.count
        return raw.enumerated().compactMap { (idx, entry) in
            parse(entry, zIndex: total - idx)
        }
    }

    /// CGWindowID of `pid`'s frontmost on-screen window, or `nil` if the pid
    /// has no visible window. Uses the same "max zIndex, non-degenerate
    /// bounds" selector as `WindowCoordinateSpace.screenPoint(...)` so the
    /// window we stamp into Skylight fields matches the one our coordinate
    /// math already anchors against.
    public static func frontmostWindowID(forPid pid: Int32) -> CGWindowID? {
        guard let win = frontmostWindow(forPid: pid) else { return nil }
        return CGWindowID(win.id)
    }

    /// Full frontmost on-screen window record for `pid` — same selector as
    /// ``frontmostWindowID(forPid:)`` but returns the ``WindowInfo`` so
    /// callers that also need `bounds` (e.g. the auth-signed click recipe that
    /// computes a window-local point via `CGEventSetWindowLocation`) can
    /// read both off a single query.
    public static func frontmostWindow(forPid pid: Int32) -> WindowInfo? {
        let candidates = visibleWindows()
            .filter { $0.pid == pid && $0.isOnScreen }
            .filter { $0.bounds.width > 1 && $0.bounds.height > 1 }
        return candidates.max(by: { $0.zIndex < $1.zIndex })
    }

    private static func parse(_ entry: [String: Any], zIndex: Int) -> WindowInfo? {
        guard
            let id = entry[kCGWindowNumber as String] as? Int,
            let pidValue = entry[kCGWindowOwnerPID as String] as? Int,
            let boundsDict = entry[kCGWindowBounds as String] as? [String: Double]
        else {
            return nil
        }

        let owner = entry[kCGWindowOwnerName as String] as? String ?? ""
        let name = entry[kCGWindowName as String] as? String ?? ""
        let layer = entry[kCGWindowLayer as String] as? Int ?? 0
        let isOnScreen = entry[kCGWindowIsOnscreen as String] as? Bool ?? false

        let bounds = WindowBounds(
            x: boundsDict["X"] ?? 0,
            y: boundsDict["Y"] ?? 0,
            width: boundsDict["Width"] ?? 0,
            height: boundsDict["Height"] ?? 0
        )

        return WindowInfo(
            id: id,
            pid: Int32(pidValue),
            owner: owner,
            name: name,
            bounds: bounds,
            zIndex: zIndex,
            isOnScreen: isOnScreen,
            layer: layer
        )
    }
}
