// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CoreGraphics
import Foundation

/// Where the canvas is looking: a world point at the center of the view and a
/// zoom (screen points per world point).
///
/// World coordinates are points, y grows downward (the same orientation as the
/// streamed windows), and the world has no edges. The camera is the only state
/// pan and zoom change, so every gesture, animation and HUD reading goes
/// through these functions.
public struct Camera: Equatable, Sendable {
    public var center: CGPoint
    public var zoom: CGFloat

    public static let minZoom: CGFloat = 0.04
    public static let maxZoom: CGFloat = 4

    public init(center: CGPoint = .zero, zoom: CGFloat = 1) {
        self.center = center
        self.zoom = Self.clamp(zoom)
    }

    public static func clamp(_ zoom: CGFloat) -> CGFloat {
        guard zoom.isFinite, zoom > 0 else { return 1 }
        return min(max(zoom, minZoom), maxZoom)
    }

    /// The world rectangle visible in a view of `viewSize`.
    public func viewport(in viewSize: CGSize) -> CGRect {
        let w = viewSize.width / zoom
        let h = viewSize.height / zoom
        return CGRect(x: center.x - w / 2, y: center.y - h / 2, width: w, height: h)
    }

    public func screenPoint(forWorld p: CGPoint, in viewSize: CGSize) -> CGPoint {
        CGPoint(x: (p.x - center.x) * zoom + viewSize.width / 2,
                y: (p.y - center.y) * zoom + viewSize.height / 2)
    }

    public func worldPoint(forScreen p: CGPoint, in viewSize: CGSize) -> CGPoint {
        CGPoint(x: (p.x - viewSize.width / 2) / zoom + center.x,
                y: (p.y - viewSize.height / 2) / zoom + center.y)
    }

    public func screenRect(forWorld r: CGRect, in viewSize: CGSize) -> CGRect {
        let o = screenPoint(forWorld: r.origin, in: viewSize)
        return CGRect(x: o.x, y: o.y, width: r.width * zoom, height: r.height * zoom)
    }

    /// Pan by a screen-space delta (a trackpad scroll moves the content with
    /// the fingers, so the camera moves the other way).
    public func panned(byScreen delta: CGVector) -> Camera {
        var c = self
        c.center.x -= delta.dx / zoom
        c.center.y -= delta.dy / zoom
        return c
    }

    /// Zoom by `factor` keeping the world point under `anchor` (screen) fixed.
    public func zoomed(by factor: CGFloat, anchor: CGPoint, in viewSize: CGSize) -> Camera {
        let before = worldPoint(forScreen: anchor, in: viewSize)
        var c = self
        c.zoom = Self.clamp(zoom * factor)
        let after = c.worldPoint(forScreen: anchor, in: viewSize)
        c.center.x += before.x - after.x
        c.center.y += before.y - after.y
        return c
    }

    /// The camera that shows `rect` whole, centered, with `padding` screen
    /// points on the tighter axis.
    public static func fitting(_ rect: CGRect, in viewSize: CGSize, padding: CGFloat = 48,
                               maxZoom: CGFloat = Camera.maxZoom) -> Camera {
        guard rect.width > 0, rect.height > 0, viewSize.width > 0, viewSize.height > 0 else {
            return Camera(center: CGPoint(x: rect.midX, y: rect.midY), zoom: 1)
        }
        let aw = max(viewSize.width - padding * 2, 1)
        let ah = max(viewSize.height - padding * 2, 1)
        let z = min(aw / rect.width, ah / rect.height, maxZoom)
        return Camera(center: CGPoint(x: rect.midX, y: rect.midY), zoom: z)
    }
}

/// A camera flight between two views: the "zoom out, travel, zoom in" path of
/// van Wijk and Nuij ("Smooth and efficient zooming and panning", 2003), which
/// keeps perceived speed constant. Short hops degrade to a plain log-zoom
/// interpolation.
public struct CameraFlight: Sendable {
    public let from: Camera
    public let to: Camera
    public let viewSize: CGSize
    /// Seconds. Scales with the path length, clamped so a hop never drags and
    /// a long flight never takes over the screen.
    public let duration: TimeInterval

    private let rho: Double = 1.414
    private let u1: Double
    private let w0: Double
    private let w1: Double
    private let r0: Double
    private let length: Double
    private let isShortPath: Bool

    public init(from: Camera, to: Camera, viewSize: CGSize,
                minDuration: TimeInterval = 0.32, maxDuration: TimeInterval = 0.9) {
        self.from = from
        self.to = to
        self.viewSize = viewSize
        let dx = Double(to.center.x - from.center.x)
        let dy = Double(to.center.y - from.center.y)
        let u1 = (dx * dx + dy * dy).squareRoot()
        // Widths of the visible world at either end.
        let w0 = Double(viewSize.width / from.zoom)
        let w1 = Double(viewSize.width / to.zoom)
        self.u1 = u1
        self.w0 = w0
        self.w1 = w1
        let rho = 1.414
        if u1 < 1e-6 * max(w0, w1) || !u1.isFinite {
            isShortPath = true
            r0 = 0
            length = abs(log(w1 / w0)) / rho
        } else {
            isShortPath = false
            let b0 = (w1 * w1 - w0 * w0 + pow(rho, 4) * u1 * u1) / (2 * w0 * rho * rho * u1)
            let b1 = (w1 * w1 - w0 * w0 - pow(rho, 4) * u1 * u1) / (2 * w1 * rho * rho * u1)
            let r0 = log(-b0 + (b0 * b0 + 1).squareRoot())
            let r1 = log(-b1 + (b1 * b1 + 1).squareRoot())
            self.r0 = r0
            length = (r1 - r0) / rho
        }
        let d = 0.35 + 0.22 * length
        duration = min(max(d, minDuration), maxDuration)
    }

    /// Standard ease: slow in, slow out.
    public static func ease(_ t: Double) -> Double {
        let t = min(max(t, 0), 1)
        return t < 0.5 ? 4 * t * t * t : 1 - pow(-2 * t + 2, 3) / 2
    }

    /// The camera at `elapsed` seconds into the flight.
    public func camera(at elapsed: TimeInterval) -> Camera {
        let t = Self.ease(duration > 0 ? elapsed / duration : 1)
        if t >= 1 { return to }
        let s = t * length
        var w: Double
        var u: Double
        if isShortPath {
            let k = w1 < w0 ? -1.0 : 1.0
            w = w0 * exp(k * rho * s)
            u = u1 * t
        } else {
            let coshR0 = cosh(r0)
            w = w0 * coshR0 / cosh(rho * s + r0)
            u = w0 / (rho * rho) * (coshR0 * tanh(rho * s + r0) - sinh(r0))
        }
        if !w.isFinite || w <= 0 { w = w0 + (w1 - w0) * t }
        if !u.isFinite { u = u1 * t }
        let f = u1 > 0 ? u / u1 : t
        let center = CGPoint(x: from.center.x + (to.center.x - from.center.x) * f,
                             y: from.center.y + (to.center.y - from.center.y) * f)
        return Camera(center: center, zoom: viewSize.width / CGFloat(w))
    }
}
