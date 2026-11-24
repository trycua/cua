// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation

/// Frame-interval statistics from display-link timestamps.
///
/// A frame is "late" when its interval is more than 1.5x the display's
/// nominal interval (at 120 Hz: over 12.5 ms), which is what a user sees as a
/// hitch. Bounded: it keeps at most `capacity` samples.
public struct FrameStats: Sendable, Equatable {
    public private(set) var intervals: [Double] = []
    public let capacity: Int
    private var last: Double?

    public init(capacity: Int = 20_000) { self.capacity = capacity }

    public mutating func tick(_ timestamp: Double) {
        defer { last = timestamp }
        guard let last else { return }
        let dt = timestamp - last
        guard dt > 0, dt < 1, intervals.count < capacity else { return }
        intervals.append(dt)
    }

    public mutating func reset() {
        intervals.removeAll(keepingCapacity: true)
        last = nil
    }

    public var count: Int { intervals.count }

    public func percentile(_ p: Double) -> Double {
        guard !intervals.isEmpty else { return 0 }
        let s = intervals.sorted()
        let i = min(max(Int((p / 100 * Double(s.count - 1)).rounded()), 0), s.count - 1)
        return s[i]
    }

    public var mean: Double { intervals.isEmpty ? 0 : intervals.reduce(0, +) / Double(intervals.count) }

    public var fps: Double { mean > 0 ? 1 / mean : 0 }

    public func lateFrames(nominal: Double) -> Int {
        intervals.filter { $0 > nominal * 1.5 }.count
    }

    public func summary(nominal: Double) -> Summary {
        Summary(frames: count, meanMs: mean * 1000, p50Ms: percentile(50) * 1000,
                p95Ms: percentile(95) * 1000, p99Ms: percentile(99) * 1000,
                maxMs: (intervals.max() ?? 0) * 1000, fps: fps,
                late: lateFrames(nominal: nominal), nominalMs: nominal * 1000)
    }

    public struct Summary: Codable, Sendable, Equatable {
        public var frames: Int
        public var meanMs: Double
        public var p50Ms: Double
        public var p95Ms: Double
        public var p99Ms: Double
        public var maxMs: Double
        public var fps: Double
        public var late: Int
        public var nominalMs: Double
    }
}
