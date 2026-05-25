import AppKit
import CoreGraphics
import QuartzCore

/// Builds the motion path the agent cursor traces between two screen
/// points. Exposes the five knobs reference implementations call
/// `startHandle`, `endHandle`, `arcSize`, `arcFlow`, `spring`.
///
/// Defaults are tuned — callers can pass `CursorMotionPath.Options()`
/// and get a natural-looking arc without thinking about the math. The
/// knobs are exposed for LLM or debug-UI tuning sessions.
public struct CursorMotionPath: Sendable {
    public struct Options: Sendable, Equatable {
        /// How far along the straight line from start to end the first
        /// control point sits. 0 keeps it at the start (tight
        /// departure); 1 slides it all the way to the endpoint (floppy
        /// departure). Typical: 0.3.
        public var startHandle: CGFloat

        /// Same as ``startHandle`` but measured from the end. 0 keeps
        /// the second control point at the end (tight arrival); 1
        /// slides it all the way to the start. Typical: 0.3.
        public var endHandle: CGFloat

        /// Magnitude of the perpendicular deflection, expressed as a
        /// fraction of the path length. 0 = straight line. 0.25 =
        /// modest arc. 0.5 = dramatic swoop. Typical: 0.25.
        public var arcSize: CGFloat

        /// Asymmetry between the two control points along the
        /// perpendicular axis. Positive values bulge the arc toward
        /// the end (apex near the destination); negative values bulge
        /// toward the start (apex near the origin). Range `[-1, 1]`.
        /// Typical: 0.
        public var arcFlow: CGFloat

        /// Damping of the post-arrival spring settle, where 1.0 =
        /// critically damped (no overshoot) and 0.4 = bouncy. Range
        /// `[0.3, 1.0]`. Values below 0.3 produce oscillations that
        /// read as glitchy rather than playful. Typical: 0.72.
        public var spring: CGFloat

        public init(
            startHandle: CGFloat = 0.3,
            endHandle: CGFloat = 0.3,
            arcSize: CGFloat = 0.25,
            arcFlow: CGFloat = 0.0,
            spring: CGFloat = 0.72
        ) {
            self.startHandle = startHandle
            self.endHandle = endHandle
            self.arcSize = arcSize
            self.arcFlow = arcFlow
            self.spring = spring
        }

        public static let `default` = Options()
    }

    /// The Bezier that the cursor traces during the main glide. Spans
    /// the full distance from `from` to `to`.
    public let bezier: CubicBezier

    /// The options the Bezier was built from. Kept so downstream
    /// animators can derive the spring settle without a second copy.
    public let options: Options

    /// Build a motion path from `from` to `to` with the given knobs.
    ///
    /// Control-point placement:
    /// 1. Slide each handle along the straight line by `startHandle` /
    ///    `endHandle` fractions.
    /// 2. Offset each handle perpendicular to the line by a distance
    ///    proportional to `arcSize * pathLength`.
    /// 3. `arcFlow` biases which handle gets more perpendicular offset
    ///    — positive pushes the apex toward the destination, negative
    ///    toward the origin.
    public init(from start: CGPoint, to end: CGPoint, options: Options = .default) {
        self.options = options

        let dx = end.x - start.x
        let dy = end.y - start.y
        let length = max(hypot(dx, dy), 1)

        // Unit perpendicular (90° CCW rotation of the direction vector).
        // Flip the sign if you want the arc on the other side of the line —
        // we always arc "above" the line in macOS screen coords (which are
        // top-left origin, so "above" means smaller y).
        let perpX = -dy / length
        let perpY = dx / length

        let deflection = length * options.arcSize

        // arcFlow in [-1, 1] maps to a [0, 1] flowBias where 0 = apex
        // near start, 1 = apex near end, 0.5 = symmetric.
        let flowBias = (options.arcFlow + 1) / 2

        // Control-point deflection magnitudes. When flowBias = 0.5,
        // both get the same deflection (symmetric arc). When bias
        // skews, the far handle gets less deflection than the near
        // one, which pulls the apex along the curve.
        let c1Deflect = deflection * (1 - 0.5 * flowBias)
        let c2Deflect = deflection * (1 - 0.5 * (1 - flowBias))

        let c1Base = CGPoint(
            x: start.x + dx * options.startHandle,
            y: start.y + dy * options.startHandle
        )
        let c2Base = CGPoint(
            x: end.x - dx * options.endHandle,
            y: end.y - dy * options.endHandle
        )

        let control1 = CGPoint(
            x: c1Base.x + perpX * c1Deflect,
            y: c1Base.y + perpY * c1Deflect
        )
        let control2 = CGPoint(
            x: c2Base.x + perpX * c2Deflect,
            y: c2Base.y + perpY * c2Deflect
        )

        self.bezier = CubicBezier(
            start: start, control1: control1, control2: control2, end: end
        )
    }

    /// A keyframe animation on `position` that drives a layer along
    /// the Bezier over `duration` seconds. Uses `.paced` calculation
    /// mode so the cursor moves at constant speed along the curve
    /// (not constant parametric `t`), which reads more naturally than
    /// a raw linear-in-t animation where the cursor accelerates
    /// through tight corners.
    ///
    /// Does not install the animation — caller decides when to add it
    /// to their layer (`layer.add(anim, forKey: "glide")`).
    public func positionAnimation(duration: CFTimeInterval) -> CAKeyframeAnimation {
        let anim = CAKeyframeAnimation(keyPath: "position")
        anim.path = bezier.cgPath
        anim.duration = duration
        anim.calculationMode = .paced
        anim.rotationMode = nil  // we drive rotation separately via `rotationAnimation`
        anim.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)
        return anim
    }

    /// A spring animation on `transform.rotation.z` that settles the
    /// cursor's orientation after the main glide completes. The
    /// incoming rotation is the tangent at the arrival point; the
    /// outgoing rotation is the cursor's default resting tilt
    /// (`restingTiltRadians`).
    ///
    /// Tuned so `spring = 1.0` produces a near-instant critically
    /// damped settle and `spring = 0.4` gives a playful couple of
    /// oscillations before landing.
    public func rotationSettle(
        from arrivalTangent: CGFloat,
        restingTiltRadians: CGFloat
    ) -> CASpringAnimation {
        let spring = CASpringAnimation(keyPath: "transform.rotation.z")
        spring.fromValue = arrivalTangent
        spring.toValue = restingTiltRadians
        spring.mass = 0.5
        spring.stiffness = 120
        spring.damping = max(0.3, options.spring) * 20
        spring.initialVelocity = 0
        spring.duration = spring.settlingDuration
        return spring
    }
}
