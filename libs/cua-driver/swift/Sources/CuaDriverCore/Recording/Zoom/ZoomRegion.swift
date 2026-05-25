// Zoom-region model + click-to-region generator. The curve-sampling math is
// straight cubic-bezier (Foley/van Dam §11); chained-region merging is a
// sweep over sorted events with an inter-click gap threshold.
//
// When two or more clicks merge into one region, the focus is no longer a
// static `(focusX, focusY)` — each original click becomes a `FocusWaypoint`,
// and `sampleCurve` pans the focus between them over time. The pan uses a
// `cubic-bezier(0.1, 0, 0.2, 1)` easing: a soft initial glide, a quick
// middle traversal, and a gentle settle onto the next click. Shape matches
// what modern screen-recording editors use for connected-zoom transitions
// (OpenScreen calls this `easeConnectedPan`).
//
// This file is infrastructure for the render pipeline that will consume it
// in a later commit — it does not render anything itself.

import Foundation

/// Numeric defaults that tune the feel of the zoom timeline. Values are
/// milliseconds unless otherwise noted. Scale is a multiplier on cursor-space
/// coordinates: `1.0` is no zoom, `2.0` is 2× magnification centered on `focus`.
public enum ZoomDefaults {
    /// Duration of the zoom-out tail at the end of a region.
    public static let transitionWindowMs: Double = 1015.05
    /// Duration of the zoom-in lead at the start of a region (1.5× the tail).
    public static let zoomInWindowMs: Double = 1522.575
    /// Baseline exponential-smoothing factor for cursor follow.
    public static let smoothingFactor: Double = 0.12
    /// Fraction of the viewport the zoomed content is allowed to occupy before
    /// being clamped against the frame edges.
    public static let viewportScale: Double = 0.8
    /// Translational movement below this (in pixels) is suppressed to avoid jitter.
    public static let translationDeadzonePx: Double = 1.25
    /// Scale change below this fraction is suppressed.
    public static let scaleDeadzone: Double = 0.002
    /// Minimum adaptive smoothing factor — used when the cursor is nearly still.
    public static let autoFollowSmoothingMin: Double = 0.1
    /// Maximum adaptive smoothing factor — used when the cursor has moved
    /// `autoFollowRampDistance` or more since the last frame.
    public static let autoFollowSmoothingMax: Double = 0.25
    /// Distance (in normalized viewport units) at which the adaptive smoothing
    /// factor saturates at `autoFollowSmoothingMax`.
    public static let autoFollowRampDistance: Double = 0.15
    /// Two zoom regions whose end-to-start gap is shorter than this get merged
    /// into a single pan-between-foci region.
    public static let chainedZoomPanGapMs: Double = 1500
    /// Duration of the pan segment that smoothly moves focus between two
    /// merged zoom foci inside a chained region.
    public static let connectedZoomPanDurationMs: Double = 1000
}

/// One focus waypoint inside a merged zoom region. Each waypoint pins the
/// camera focus to `(x, y)` at playback time `tMs`; `sampleCurve` interpolates
/// between surrounding waypoints so the camera pans smoothly from one click
/// target to the next instead of holding on the first.
public struct FocusWaypoint: Sendable, Codable, Equatable {
    public let tMs: Double
    public let x: Double
    public let y: Double

    public init(tMs: Double, x: Double, y: Double) {
        self.tMs = tMs
        self.x = x
        self.y = y
    }
}

/// A single zoom region on the playback timeline.
///
/// A region covers `[startMs, endMs]` and has three phases:
///   1. Zoom-in — `[startMs, startMs + zoomInDurationMs]`, scale 1.0 → `scale`
///   2. Hold    — between the two transitions, scale held at `scale`
///   3. Zoom-out — `[endMs - zoomOutDurationMs, endMs]`, scale `scale` → 1.0
///
/// If the two transition windows exceed the total duration, both are scaled
/// down proportionally so they fit exactly.
///
/// `waypoints` is populated when two or more clicks are merged into one
/// region (see `ZoomRegionGenerator.generate`). When non-nil and non-empty,
/// `sampleCurve` pans the focus between the waypoints over time; when nil
/// it falls back to the static `(focusX, focusY)` pair for back-compat.
public struct ZoomRegion: Sendable, Codable, Equatable {
    public let startMs: Double
    public let endMs: Double
    public let focusX: Double
    public let focusY: Double
    public let scale: Double
    public let zoomInDurationMs: Double
    public let zoomOutDurationMs: Double
    public let waypoints: [FocusWaypoint]?

    public init(
        startMs: Double,
        endMs: Double,
        focusX: Double,
        focusY: Double,
        scale: Double,
        zoomInDurationMs: Double = ZoomDefaults.zoomInWindowMs,
        zoomOutDurationMs: Double = ZoomDefaults.transitionWindowMs,
        waypoints: [FocusWaypoint]? = nil
    ) {
        self.startMs = startMs
        self.endMs = endMs
        self.focusX = focusX
        self.focusY = focusY
        self.scale = scale
        self.zoomInDurationMs = zoomInDurationMs
        self.zoomOutDurationMs = zoomOutDurationMs
        self.waypoints = waypoints
    }

    // Explicit Codable wiring so old JSON (without `waypoints`) decodes cleanly
    // as `waypoints = nil`.
    private enum CodingKeys: String, CodingKey {
        case startMs, endMs, focusX, focusY, scale
        case zoomInDurationMs, zoomOutDurationMs, waypoints
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        self.startMs = try c.decode(Double.self, forKey: .startMs)
        self.endMs = try c.decode(Double.self, forKey: .endMs)
        self.focusX = try c.decode(Double.self, forKey: .focusX)
        self.focusY = try c.decode(Double.self, forKey: .focusY)
        self.scale = try c.decode(Double.self, forKey: .scale)
        self.zoomInDurationMs = try c.decode(Double.self, forKey: .zoomInDurationMs)
        self.zoomOutDurationMs = try c.decode(Double.self, forKey: .zoomOutDurationMs)
        self.waypoints = try c.decodeIfPresent([FocusWaypoint].self, forKey: .waypoints)
    }

    /// Returns the effective zoom-in / zoom-out durations after shrinking to
    /// fit within the region's total length, if necessary.
    public var effectiveDurations: (zoomIn: Double, zoomOut: Double) {
        let duration = max(0, endMs - startMs)
        var zoomIn = zoomInDurationMs
        var zoomOut = zoomOutDurationMs
        let combined = zoomIn + zoomOut
        if combined > duration && combined > 0 {
            let shrink = duration / combined
            zoomIn *= shrink
            zoomOut *= shrink
        }
        return (zoomIn, zoomOut)
    }
}

/// One click event on the timeline. Coordinates are in the same cursor-space
/// as `CursorSample` (top-left origin, y-down).
public struct ClickEvent: Sendable, Equatable, Codable {
    public let tMs: Double
    public let x: Double
    public let y: Double

    public init(tMs: Double, x: Double, y: Double) {
        self.tMs = tMs
        self.x = x
        self.y = y
    }
}

public enum ZoomRegionGenerator {
    /// Zoom-in starts this many ms before the click so the camera's
    /// perceived peak roughly aligns with the click moment. The bezier
    /// curve `(0.16, 1, 0.3, 1)` is aggressively ease-out — ~80% of the
    /// visible motion happens in the first ~28% of the duration
    /// (~425ms of a 1522ms zoom-in). Setting lead-in to ~600ms puts
    /// the perceived peak ~175ms before the click (slight anticipation,
    /// tail settles invisibly after). Longer lead-ins read as predictive
    /// ("zoom happened before the user could possibly know where to
    /// click"); shorter lead-ins read as reactive/sluggish.
    private static let preClickLeadInMs: Double = 600
    /// Hold this many ms after a click before starting the zoom-out.
    private static let postClickHoldMs: Double = 1500

    /// Converts a sequence of click events into a timeline of zoom regions.
    ///
    /// Clicks whose default regions would sit within
    /// `ZoomDefaults.chainedZoomPanGapMs` of each other are merged into a
    /// single region that spans both; the focus of the merged region will pan
    /// between the two click points when sampled via `sampleCurve`.
    ///
    /// `cursorSamples` is accepted for API parity with `sampleCurve` — current
    /// generation logic does not consume it, but a future commit may use it
    /// to place the focus on the cursor-predicted landing point rather than
    /// the raw click coordinate.
    public static func generate(
        clicks: [ClickEvent],
        cursorSamples: [CursorSample],
        defaultScale: Double = 2.0
    ) -> [ZoomRegion] {
        _ = cursorSamples  // reserved for cursor-aware focus placement

        let sorted = clicks.sorted { $0.tMs < $1.tMs }
        var out: [ZoomRegion] = []
        out.reserveCapacity(sorted.count)

        for click in sorted {
            let seed = ZoomRegion(
                startMs: click.tMs - preClickLeadInMs,
                endMs: click.tMs + postClickHoldMs,
                focusX: click.x,
                focusY: click.y,
                scale: defaultScale,
                zoomInDurationMs: ZoomDefaults.zoomInWindowMs,
                zoomOutDurationMs: ZoomDefaults.transitionWindowMs,
                waypoints: nil
            )
            let seedWaypoint = FocusWaypoint(tMs: click.tMs, x: click.x, y: click.y)

            // Merge with the tail region if the gap between them is shorter
            // than the chained-zoom threshold. The merged region spans the
            // earlier start to the later end; its focus pans between each
            // original click via the `waypoints` list, resolved at sample
            // time by `sampleCurve`.
            if let last = out.last,
               seed.startMs - last.endMs < ZoomDefaults.chainedZoomPanGapMs {
                let existing = last.waypoints ?? [
                    FocusWaypoint(tMs: last.startMs + preClickLeadInMs,
                                  x: last.focusX, y: last.focusY)
                ]
                let merged = ZoomRegion(
                    startMs: last.startMs,
                    endMs: seed.endMs,
                    focusX: last.focusX,
                    focusY: last.focusY,
                    scale: max(last.scale, seed.scale),
                    zoomInDurationMs: last.zoomInDurationMs,
                    zoomOutDurationMs: seed.zoomOutDurationMs,
                    waypoints: existing + [seedWaypoint]
                )
                out[out.count - 1] = merged
            } else {
                out.append(seed)
            }
        }

        return out
    }

    /// Samples the zoom curve at playback time `t`. Returns the instantaneous
    /// scale plus the focus point to center the zoom on.
    ///
    /// If `t` is inside a region, scale animates 1.0 → `region.scale` → 1.0
    /// across the three phases using a smooth ease curve. Focus resolves
    /// via `resolveFocus`: a multi-waypoint region pans between waypoints
    /// using `easeConnectedPan`; a single-focus region returns the static
    /// `(focusX, focusY)` pair.
    ///
    /// If `t` is outside every region, scale is 1.0 and focus is the current
    /// cursor position (or (0, 0) if there is no cursor data).
    public static func sampleCurve(
        atMs t: Double,
        regions: [ZoomRegion],
        cursorSamples: [CursorSample]
    ) -> (scale: Double, focusX: Double, focusY: Double) {
        // Resting state: follow the cursor with no magnification.
        let resting: (Double, Double, Double) = {
            if let p = CursorTelemetry.positionAt(timeMs: t, samples: cursorSamples) {
                return (1.0, p.x, p.y)
            }
            return (1.0, 0, 0)
        }()

        guard let region = regions.first(where: { t >= $0.startMs && t <= $0.endMs }) else {
            return resting
        }

        let (zoomIn, zoomOut) = region.effectiveDurations
        let inEnd = region.startMs + zoomIn
        let outStart = region.endMs - zoomOut

        // Progress through the active phase, 0 → 1.
        // Curve is `cubic-bezier(0.16, 1, 0.3, 1)` — a strong ease-out with a
        // near-linear settle tail. Reads as a confident "snap to target" for
        // zoom-in and a soft release for zoom-out. Same shape used by common
        // screen-recording UX (matches Screen Studio feel).
        let easeX1 = 0.16, easeY1 = 1.0, easeX2 = 0.3, easeY2 = 1.0
        let phaseProgress: Double
        if t < inEnd {
            // Zoom-in phase.
            let span = max(1e-6, zoomIn)
            phaseProgress = cubicBezier(
                x1: easeX1, y1: easeY1, x2: easeX2, y2: easeY2,
                t: clamp01((t - region.startMs) / span))
        } else if t > outStart {
            // Zoom-out phase. Compute 1 → 0 across the tail and invert to get
            // the eased scale factor.
            let span = max(1e-6, zoomOut)
            let tail = clamp01((region.endMs - t) / span)
            phaseProgress = cubicBezier(
                x1: easeX1, y1: easeY1, x2: easeX2, y2: easeY2, t: tail)
        } else {
            // Hold phase: fully zoomed.
            phaseProgress = 1
        }

        let scale = lerp(1.0, region.scale, phaseProgress)
        let (focusX, focusY) = resolveFocus(atMs: t, region: region)
        return (scale, focusX, focusY)
    }

    /// Resolves the focus point for a region at playback time `t`.
    ///
    /// If the region has a non-empty `waypoints` list, the focus is lerped
    /// between the two waypoints surrounding `t` using an `easeConnectedPan`
    /// curve (`cubic-bezier(0.1, 0, 0.2, 1)`). Before the first waypoint the
    /// focus is clamped to the first; after the last it is clamped to the
    /// last. If `waypoints` is nil or empty, the static `(focusX, focusY)`
    /// pair is returned — preserving back-compat for unmerged regions and
    /// older decoded data.
    private static func resolveFocus(atMs t: Double, region: ZoomRegion)
        -> (x: Double, y: Double)
    {
        guard let waypoints = region.waypoints, !waypoints.isEmpty else {
            return (region.focusX, region.focusY)
        }

        if waypoints.count == 1 || t <= waypoints[0].tMs {
            let w = waypoints[0]
            return (w.x, w.y)
        }
        if let last = waypoints.last, t >= last.tMs {
            return (last.x, last.y)
        }

        // Walk adjacent pairs to find the bracketing waypoints.
        for i in 0..<(waypoints.count - 1) {
            let a = waypoints[i]
            let b = waypoints[i + 1]
            guard t >= a.tMs && t <= b.tMs else { continue }
            let span = max(1e-6, b.tMs - a.tMs)
            let localT = clamp01((t - a.tMs) / span)
            // easeConnectedPan: cubic-bezier(0.1, 0, 0.2, 1) — see file header.
            let eased = cubicBezier(x1: 0.1, y1: 0, x2: 0.2, y2: 1, t: localT)
            return (lerp(a.x, b.x, eased), lerp(a.y, b.y, eased))
        }

        // Unreachable under the clamps above, but stay defensive.
        return (region.focusX, region.focusY)
    }
}
