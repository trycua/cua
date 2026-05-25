import AppKit
import CoreGraphics
import Foundation

public enum WindowCoordinateSpaceError: Error, CustomStringConvertible, Sendable {
    case noVisibleWindow(pid: Int32)
    case windowNotFound(windowId: UInt32)
    case windowNotOwnedByPid(windowId: UInt32, ownerPid: Int32, requestedPid: Int32)

    public var description: String {
        switch self {
        case .noVisibleWindow(let pid):
            return "pid \(pid) has no resolvable window — cannot translate window-local pixel to screen point. Snapshot the target first."
        case .windowNotFound(let windowId):
            return "No window with window_id \(windowId) exists; cannot translate window-local pixel to screen point."
        case .windowNotOwnedByPid(let windowId, let ownerPid, let requestedPid):
            return "window_id \(windowId) belongs to pid \(ownerPid), not pid \(requestedPid)."
        }
    }
}

/// Converts window-local screenshot pixel coordinates (top-left origin of the
/// target window's image) to screen points (top-left origin of the macOS
/// global desktop, AX/CGEvent convention).
///
/// Rationale: our `click({pid, x, y})` and `right_click({pid, x, y})` accept
/// coordinates in the same space as the screenshot returned by
/// `get_window_state`. This matches the convention used by most
/// computer-use tools in the space — agents read a coordinate off the
/// screenshot and pass it through without manual conversion. The driver
/// does the window-origin + backing-scale math internally.
public enum WindowCoordinateSpace {
    /// Translate an image-pixel coordinate into a screen-point coordinate
    /// using the pid's frontmost window as the anchor. Prefer the
    /// `window_id`-scoped variant when the caller knows which window
    /// produced the screenshot (e.g. from a `get_window_state` turn);
    /// this variant exists for pixel-only callers that never read the
    /// AX tree.
    ///
    /// Window selection matches `WindowCapture.selectFrontmostWindow`:
    /// visible + on-current-Space first, fall back to max-area. That
    /// rule also applies to pixel-path recording and the tool-layer
    /// click-point marker, so a screenshot and the conversion anchor
    /// agree on which window they're talking about.
    public static func screenPoint(
        fromImagePixel imagePixel: CGPoint,
        forPid pid: Int32
    ) throws -> CGPoint {
        guard let target = WindowCapture.selectFrontmostWindow(forPid: pid)
        else {
            throw WindowCoordinateSpaceError.noVisibleWindow(pid: pid)
        }
        return convert(imagePixel: imagePixel, windowBounds: target.bounds)
    }

    /// Translate an image-pixel coordinate into a screen-point using a
    /// specific `windowId` as the anchor — used when the caller already
    /// knows exactly which window they screenshot'd (the common case
    /// after `get_window_state(pid, window_id)`). Validates that the
    /// window exists and belongs to the pid before computing.
    public static func screenPoint(
        fromImagePixel imagePixel: CGPoint,
        forPid pid: Int32,
        windowId: UInt32
    ) throws -> CGPoint {
        guard let info = WindowEnumerator.allWindows().first(where: {
            UInt32($0.id) == windowId
        }) else {
            throw WindowCoordinateSpaceError.windowNotFound(windowId: windowId)
        }
        if info.pid != pid {
            throw WindowCoordinateSpaceError.windowNotOwnedByPid(
                windowId: windowId, ownerPid: info.pid, requestedPid: pid)
        }
        return convert(imagePixel: imagePixel, windowBounds: info.bounds)
    }

    private static func convert(
        imagePixel: CGPoint, windowBounds: WindowBounds
    ) -> CGPoint {
        let scale = backingScale(for: windowBounds)
        return CGPoint(
            x: windowBounds.x + imagePixel.x / scale,
            y: windowBounds.y + imagePixel.y / scale
        )
    }

    /// Backing scale factor for the screen the window lives on. Matches the
    /// same logic `WindowCapture.scaleFactor(for:)` uses so the conversion
    /// cancels out the scale baked into the screenshot.
    private static func backingScale(for bounds: WindowBounds) -> CGFloat {
        let frame = CGRect(
            x: bounds.x, y: bounds.y,
            width: bounds.width, height: bounds.height
        )
        var best: NSScreen? = nil
        var bestArea: CGFloat = 0
        for screen in NSScreen.screens {
            let intersection = screen.frame.intersection(frame)
            guard !intersection.isNull else { continue }
            let area = intersection.width * intersection.height
            if area > bestArea {
                bestArea = area
                best = screen
            }
        }
        return (best ?? NSScreen.main)?.backingScaleFactor ?? 1.0
    }
}
