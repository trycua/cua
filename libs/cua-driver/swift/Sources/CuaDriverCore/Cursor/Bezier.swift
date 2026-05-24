import CoreGraphics
import Foundation

/// A cubic Bezier curve sampled parametrically over `t ∈ [0, 1]`. Used
/// as the path the agent cursor traces when gliding from one point to
/// another.
///
/// `control1` and `control2` are the tangent handles; with `arcSize`
/// and `arcFlow` builders in ``CursorMotionPath`` these translate to
/// the perpendicular-offset handles debug sliders expose as START /
/// END handles in reference implementations.
public struct CubicBezier: Sendable, Equatable {
    public let start: CGPoint
    public let control1: CGPoint
    public let control2: CGPoint
    public let end: CGPoint

    public init(
        start: CGPoint, control1: CGPoint, control2: CGPoint, end: CGPoint
    ) {
        self.start = start
        self.control1 = control1
        self.control2 = control2
        self.end = end
    }

    /// The curve point at parametric `t` in `[0, 1]`.
    /// `B(t) = (1-t)³·P0 + 3(1-t)²t·P1 + 3(1-t)t²·P2 + t³·P3`
    public func point(at t: CGFloat) -> CGPoint {
        let u = 1 - t
        let uu = u * u
        let uuu = uu * u
        let tt = t * t
        let ttt = tt * t
        return CGPoint(
            x: uuu * start.x + 3 * uu * t * control1.x + 3 * u * tt * control2.x
                + ttt * end.x,
            y: uuu * start.y + 3 * uu * t * control1.y + 3 * u * tt * control2.y
                + ttt * end.y
        )
    }

    /// The tangent angle (in radians) at parametric `t`. Useful when
    /// an animated cursor should rotate to face the direction of
    /// travel — set `transform = CATransform3DMakeRotation(...)` with
    /// this angle during the flight.
    ///
    /// `B'(t) = 3(1-t)²(P1-P0) + 6(1-t)t(P2-P1) + 3t²(P3-P2)`
    public func tangentRadians(at t: CGFloat) -> CGFloat {
        let u = 1 - t
        let dx =
            3 * u * u * (control1.x - start.x)
            + 6 * u * t * (control2.x - control1.x)
            + 3 * t * t * (end.x - control2.x)
        let dy =
            3 * u * u * (control1.y - start.y)
            + 6 * u * t * (control2.y - control1.y)
            + 3 * t * t * (end.y - control2.y)
        return atan2(dy, dx)
    }

    /// A `CGPath` representing the full curve from start to end. Use
    /// this as the `path` of a `CAKeyframeAnimation` to drive a
    /// CALayer along the curve without sampling manually.
    public var cgPath: CGPath {
        let path = CGMutablePath()
        path.move(to: start)
        path.addCurve(to: end, control1: control1, control2: control2)
        return path
    }
}
