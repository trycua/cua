// Cursor-timeline lookup + exponential smoothing. Algorithms are textbook:
// binary-search-then-lerp on a sorted timeline, and classic exponential
// smoothing (prev + α·(raw - prev)) — see any intro signal-processing text.
//
// Coordinates are cua-driver's screen-points convention: top-left origin,
// y-down, units matching what `CursorSampler` writes to `cursor.jsonl`.

import Foundation

/// One cursor-position sample drawn from `cursor.jsonl`. Timestamps are
/// milliseconds relative to session start (same anchor as `session.json`'s
/// `started_at_monotonic_ns`).
public struct CursorSample: Sendable, Codable, Equatable {
    public let tMs: Double
    public let x: Double
    public let y: Double

    public init(tMs: Double, x: Double, y: Double) {
        self.tMs = tMs
        self.x = x
        self.y = y
    }
}

/// 2D position in cursor-space. Same top-left-origin convention as `CursorSample`.
public struct CursorPosition: Sendable, Equatable, Codable {
    public let x: Double
    public let y: Double

    public init(x: Double, y: Double) {
        self.x = x
        self.y = y
    }
}

public enum CursorTelemetry {
    /// Looks up the cursor position at `timeMs`, clamped to the sample range.
    /// `samples` must be sorted ascending by `tMs`. Returns `nil` only for an
    /// empty array.
    public static func positionAt(timeMs: Double, samples: [CursorSample]) -> CursorPosition? {
        guard let first = samples.first, let last = samples.last else { return nil }

        if timeMs <= first.tMs {
            return CursorPosition(x: first.x, y: first.y)
        }
        if timeMs >= last.tMs {
            return CursorPosition(x: last.x, y: last.y)
        }

        // Binary-search for the right interval: find [lo, hi] with
        // samples[lo].tMs <= timeMs < samples[hi].tMs and hi == lo + 1.
        var lo = 0
        var hi = samples.count - 1
        while hi - lo > 1 {
            let mid = (lo + hi) >> 1
            if samples[mid].tMs <= timeMs {
                lo = mid
            } else {
                hi = mid
            }
        }

        let before = samples[lo]
        let after = samples[hi]
        let span = after.tMs - before.tMs
        let u = span > 0 ? (timeMs - before.tMs) / span : 0

        return CursorPosition(
            x: lerp(before.x, after.x, u),
            y: lerp(before.y, after.y, u)
        )
    }

    /// Classic exponential smoothing: `prev + factor · (raw - prev)`. A `factor`
    /// of 0 means "never move"; 1 means "snap to raw instantly".
    public static func smooth(
        _ raw: CursorPosition,
        towards prev: CursorPosition,
        factor: Double
    ) -> CursorPosition {
        let f = clamp01(factor)
        return CursorPosition(
            x: prev.x + (raw.x - prev.x) * f,
            y: prev.y + (raw.y - prev.y) * f
        )
    }

    /// Adaptive smoothing factor that scales linearly with the euclidean
    /// distance the cursor has moved. A cursor sitting still barely follows
    /// (`minFactor`), a cursor leaping across the screen snaps faster
    /// (`maxFactor`). `rampDistance` is the distance at which the factor
    /// saturates at `maxFactor`.
    public static func adaptiveFactor(
        distance: Double,
        rampDistance: Double,
        minFactor: Double,
        maxFactor: Double
    ) -> Double {
        guard rampDistance > 0 else { return maxFactor }
        let u = clamp01(distance / rampDistance)
        return minFactor + (maxFactor - minFactor) * u
    }
}
