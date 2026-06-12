import CoreGraphics

public enum CursorControl {
    /// Current mouse cursor position in screen points, origin top-left.
    /// Uses `CGEvent(source: nil).location` — this reads current state and
    /// does NOT post a synthetic event.
    public static func currentPosition() -> CGPoint {
        guard let event = CGEvent(source: nil) else { return .zero }
        return event.location
    }

    /// Instantly warp the mouse cursor to the given screen point.
    ///
    /// `CGWarpMouseCursorPosition` is a positioning API, not CGEvent
    /// synthesis — no press/release events are posted. Pair with
    /// `CGAssociateMouseAndMouseCursorPosition(1)` so the cursor stays
    /// attached to the mouse after the warp (some apps otherwise decouple).
    public static func move(to point: CGPoint) {
        CGWarpMouseCursorPosition(point)
        CGAssociateMouseAndMouseCursorPosition(1)
    }
}
