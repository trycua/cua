// Standard cubic-bezier solver. Newton-Raphson + binary-search hybrid — see Foley/van Dam §11.
// Pure math layer for the zoom-render post-process. Zero dependencies beyond Foundation.

import Foundation

/// Clamps a scalar to `[0, 1]`.
@inlinable
public func clamp01(_ v: Double) -> Double {
    if v < 0 { return 0 }
    if v > 1 { return 1 }
    return v
}

/// Linear interpolation between `a` and `b`. `amount` is not clamped — callers
/// that need clamping should wrap with `clamp01`.
@inlinable
public func lerp(_ a: Double, _ b: Double, _ amount: Double) -> Double {
    return a + (b - a) * amount
}

/// Evaluates one axis of a parametric cubic bezier with endpoints fixed at 0 and 1
/// and control-point ordinate `a1` (at t=1/3) and `a2` (at t=2/3).
@inlinable
func sampleBezier1D(_ a1: Double, _ a2: Double, _ t: Double) -> Double {
    let u = 1 - t
    return 3 * a1 * u * u * t
         + 3 * a2 * u * t * t
         + t * t * t
}

/// Derivative of `sampleBezier1D` with respect to `t`.
@inlinable
func sampleBezier1DDerivative(_ a1: Double, _ a2: Double, _ t: Double) -> Double {
    let u = 1 - t
    return 3 * a1 * u * u
         + 6 * (a2 - a1) * u * t
         + 3 * (1 - a2) * t * t
}

/// Solves a 2D cubic bezier `(x1,y1)-(x2,y2)` as a CSS-style timing function:
/// given `t` interpreted as the progress along the x-axis, return the corresponding
/// y-value. Internally inverts the parametric x(s) equation to find the curve
/// parameter `s` and then evaluates y(s).
///
/// Uses Newton-Raphson (up to 8 iterations) with a binary-search fallback
/// (up to 10 iterations) for cases where the derivative is near-zero.
public func cubicBezier(x1: Double, y1: Double, x2: Double, y2: Double, t: Double) -> Double {
    let targetX = clamp01(t)

    // Initial guess: since t ∈ [0, 1] and x is monotonically increasing on that
    // interval for sane control points, use t itself as the first estimate of s.
    var s = targetX

    // Newton-Raphson: refine s such that sampleBezier1D(x1, x2, s) == targetX.
    for _ in 0..<8 {
        let residual = sampleBezier1D(x1, x2, s) - targetX
        let slope = sampleBezier1DDerivative(x1, x2, s)
        if abs(residual) < 1e-6 { break }
        if abs(slope) < 1e-6 { break }
        s -= residual / slope
    }

    // Binary-search fallback to clean up any remaining error and guarantee
    // `s` stays in [0, 1] even on pathological control-point configurations.
    s = clamp01(s)
    var lower: Double = 0
    var upper: Double = 1
    for _ in 0..<10 {
        let x = sampleBezier1D(x1, x2, s)
        if abs(x - targetX) < 1e-6 { break }
        if x < targetX { lower = s } else { upper = s }
        s = (lower + upper) / 2
    }

    return sampleBezier1D(y1, y2, s)
}

/// Exponential ease-out: `1 - 2^(-7t)`, clamped and with the `t=1` edge case
/// pinned at exactly `1`. Produces a curve that starts fast and settles smoothly.
public func easeOutExpo(_ t: Double) -> Double {
    let c = clamp01(t)
    if c >= 1 { return 1 }
    return 1 - pow(2, -7 * c)
}
