// Action-span model: each recorded click/type produces a span covering
// its start → end times with the frontmost window's bounds at action
// time. Post-processing uses spans to drive variable-speed rendering
// (1× inside, 5× outside) and to compute per-action window-bbox zoom.
//
// Spans are padded ±1 s then merged so adjacent actions inside the
// same edit session stay at 1× with a smooth zoom that covers every
// affected window.

import Foundation

// MARK: - ActionSpan ---------------------------------------------------

/// Screen-space click coordinate (points). Stored separately from
/// `WindowBounds` so the renderer can fall back to click-point zoom
/// when the window is larger than the video frame.
public struct ClickPoint: Sendable, Equatable {
    public let x: Double
    public let y: Double
    public init(x: Double, y: Double) { self.x = x; self.y = y }
}

/// One recorded action's timespan + the window it targeted.
public struct ActionSpan: Sendable, Equatable {
    /// Session-monotonic start time of the action (ms).
    public let startMs: Double
    /// Session-monotonic end time of the action (ms).
    public let endMs: Double
    /// Frontmost window at action time (screen points, top-left origin).
    /// Nil when the action had no associated window (e.g. press_key with
    /// no pid, or an older recording without window_bounds in action.json).
    public let windowBounds: WindowBounds?
    /// Screen-space click coordinate (points) resolved by the driver at
    /// dispatch time. Present on click-class actions; nil otherwise.
    /// Used as fallback zoom focus when `windowBounds` is too large to
    /// fit inside the video frame at any scale ≥ 1×.
    public let clickPoint: ClickPoint?
    /// Per-action focus waypoints, populated when two or more raw spans
    /// are merged into one. Each waypoint records the focus position
    /// (screen points) at its action's midpoint time so `zoomRegions`
    /// can pan the camera smoothly between actions within a merged span.
    /// Nil for single-action (unmerged) spans.
    public let focusWaypoints: [FocusWaypoint]?

    public init(
        startMs: Double,
        endMs: Double,
        windowBounds: WindowBounds?,
        clickPoint: ClickPoint? = nil,
        focusWaypoints: [FocusWaypoint]? = nil
    ) {
        self.startMs = startMs
        self.endMs = endMs
        self.windowBounds = windowBounds
        self.clickPoint = clickPoint
        self.focusWaypoints = focusWaypoints
    }
}

// MARK: - ActionSpanGenerator ------------------------------------------

public enum ActionSpanGenerator {
    /// Padding added before and after each raw action span (milliseconds).
    public static let padMs: Double = 500

    /// Speed multiplier applied outside action spans in the rendered video.
    public static let fastSpeed: Double = 8.0

    /// Gap threshold: two padded spans closer than this are merged into one.
    private static let mergeGapMs: Double = 5_000

    // MARK: Span generation

    /// Pad each raw action span by ±`padMs`, then merge overlapping spans.
    /// Merged spans inherit the first span's window bounds (the dominant
    /// target window for the action cluster) and accumulate per-action
    /// focus waypoints so the camera pans smoothly between actions.
    public static func generate(from rawSpans: [ActionSpan]) -> [ActionSpan] {
        let padded = rawSpans.map {
            ActionSpan(
                startMs: max(0, $0.startMs - padMs),
                endMs: $0.endMs + padMs,
                windowBounds: $0.windowBounds,
                clickPoint: $0.clickPoint
            )
        }
        return merge(padded.sorted { $0.startMs < $1.startMs })
    }

    /// The best single focus point for a span, in screen points.
    /// Prefers the click point; falls back to the window centre.
    private static func focusForSpan(_ span: ActionSpan) -> (x: Double, y: Double)? {
        if let cp = span.clickPoint { return (cp.x, cp.y) }
        if let wb = span.windowBounds { return (wb.x + wb.width / 2, wb.y + wb.height / 2) }
        return nil
    }

    private static func merge(_ sorted: [ActionSpan]) -> [ActionSpan] {
        var result: [ActionSpan] = []
        for span in sorted {
            if let last = result.last, span.startMs <= last.endMs + mergeGapMs {
                // Midpoint ≈ original action time (padding is symmetric).
                let spanMid = (span.startMs + span.endMs) / 2

                // Bootstrap waypoints from `last` on the first merge.
                var waypoints = last.focusWaypoints ?? []
                if waypoints.isEmpty, let lf = focusForSpan(last) {
                    let lastMid = (last.startMs + last.endMs) / 2
                    waypoints = [FocusWaypoint(tMs: lastMid, x: lf.x, y: lf.y)]
                }
                if let sf = focusForSpan(span) {
                    waypoints.append(FocusWaypoint(tMs: spanMid, x: sf.x, y: sf.y))
                }

                result[result.count - 1] = ActionSpan(
                    startMs: last.startMs,
                    endMs: max(last.endMs, span.endMs),
                    windowBounds: last.windowBounds ?? span.windowBounds,
                    clickPoint: last.clickPoint ?? span.clickPoint,
                    focusWaypoints: waypoints.isEmpty ? nil : waypoints
                )
            } else {
                result.append(span)
            }
        }
        return result
    }

    // MARK: PTS remapping

    /// Map an input video timestamp `inputMs` to an output timestamp that
    /// plays spans at 1× speed and non-span segments at `fastSpeed`×.
    /// The mapping is piecewise-linear and monotonically increasing.
    public static func mapPts(_ inputMs: Double, spans: [ActionSpan]) -> Double {
        var outMs: Double = 0
        var prevEnd: Double = 0

        for span in spans {
            if span.startMs > prevEnd {
                // Fast segment before this span
                let segEnd = min(span.startMs, inputMs)
                outMs += (segEnd - prevEnd) / fastSpeed
                if inputMs <= span.startMs { return outMs }
            }
            // 1× span segment
            let start = max(span.startMs, prevEnd)
            let end = min(span.endMs, inputMs)
            if end > start { outMs += end - start }
            if inputMs <= span.endMs { return outMs }
            prevEnd = span.endMs
        }

        // Fast segment after all spans
        if inputMs > prevEnd {
            outMs += (inputMs - prevEnd) / fastSpeed
        }
        return outMs
    }

    // MARK: Lookup helpers

    /// True when `ms` falls inside any action span.
    public static func isInSpan(_ ms: Double, spans: [ActionSpan]) -> Bool {
        spans.contains { ms >= $0.startMs && ms <= $0.endMs }
    }

    /// The first span that contains `ms`, or nil.
    public static func span(at ms: Double, spans: [ActionSpan]) -> ActionSpan? {
        spans.first { ms >= $0.startMs && ms <= $0.endMs }
    }
}
