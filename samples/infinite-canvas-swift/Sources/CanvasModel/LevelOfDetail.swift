// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CoreGraphics
import Foundation

/// How much of a window's stream a tile needs right now.
///
/// Ordered: a higher tier is never cheaper. Each tier maps to the per-window
/// stream preferences the cua SDK's media session accepts at runtime
/// (`set_stream_preferences`: `max_fps`, `max_dimension`), so the Space's
/// encoder does less work, not just this client's decoder.
public enum StreamTier: Int, Comparable, Sendable, CaseIterable, CustomStringConvertible {
    /// Off screen: the server sends one small frame a second and the client
    /// does not decode it. The tile keeps its last picture.
    case paused
    /// A few pixels tall: a thumbnail at a low rate.
    case thumbnail
    case low
    case medium
    /// Large on screen, or the tile has input.
    case full

    public static func < (a: StreamTier, b: StreamTier) -> Bool { a.rawValue < b.rawValue }

    public var preferences: StreamPreferences {
        switch self {
        case .paused: return StreamPreferences(maxFps: 1, maxDimension: 160)
        case .thumbnail: return StreamPreferences(maxFps: 4, maxDimension: 360)
        case .low: return StreamPreferences(maxFps: 12, maxDimension: 720)
        case .medium: return StreamPreferences(maxFps: 30, maxDimension: 1280)
        case .full: return StreamPreferences(maxFps: 60, maxDimension: 2560)
        }
    }

    /// Whether the client decodes frames at this tier.
    public var decodes: Bool { self != .paused }

    public var description: String {
        switch self {
        case .paused: return "paused"
        case .thumbnail: return "thumbnail"
        case .low: return "low"
        case .medium: return "medium"
        case .full: return "full"
        }
    }
}

/// The per-window options sent to the Space (`set_stream_preferences`).
public struct StreamPreferences: Equatable, Sendable {
    public var maxFps: UInt16
    /// Long-edge cap in pixels.
    public var maxDimension: UInt32

    public init(maxFps: UInt16, maxDimension: UInt32) {
        self.maxFps = maxFps
        self.maxDimension = maxDimension
    }
}

/// What the policy knows about one tile at one moment.
public struct TileVisibility: Equatable, Sendable {
    /// The tile's rectangle on screen, in backing pixels (points times the
    /// display scale).
    public var screenRect: CGRect
    /// The visible screen area, in the same pixels.
    public var screenBounds: CGRect
    /// The tile has keyboard and pointer input.
    public var focused: Bool

    public init(screenRect: CGRect, screenBounds: CGRect, focused: Bool = false) {
        self.screenRect = screenRect
        self.screenBounds = screenBounds
        self.focused = focused
    }

    /// How big the tile is drawn: its longest on-screen edge, in pixels.
    public var drawnLongEdge: CGFloat { max(screenRect.width, screenRect.height) }

    /// Whether any of the tile (plus a margin, so a tile about to scroll in
    /// is already warm) is on screen.
    public func isOnScreen(margin: CGFloat) -> Bool {
        screenRect.intersects(screenBounds.insetBy(dx: -margin, dy: -margin))
    }
}

/// Decides each tile's tier from how large it is drawn, with hysteresis so a
/// tile hovering at a threshold does not flap, and a hold before stepping
/// down so a zoom that passes through a size does not thrash the encoder.
///
/// Upgrades are immediate and ask the server for a keyframe: a tile that was
/// paused holds no reference frames, and one that grows wants a sharp frame
/// now, not at the next natural IDR.
public struct LODPolicy: Sendable {
    /// Upper bounds of drawn long edge (pixels) for each tier below `.full`.
    public var thumbnailBelow: CGFloat = 220
    public var lowBelow: CGFloat = 560
    public var mediumBelow: CGFloat = 1200
    /// A downgrade needs the size to fall this far below the threshold.
    public var hysteresis: CGFloat = 0.82
    /// And to stay there this long.
    public var downgradeHold: TimeInterval = 0.45
    /// Screen margin that still counts as on screen.
    public var margin: CGFloat = 240

    public init() {}

    /// The tier the size alone asks for (no hysteresis).
    public func rawTier(for v: TileVisibility) -> StreamTier {
        if v.focused { return .full }
        guard v.isOnScreen(margin: margin) else { return .paused }
        let e = v.drawnLongEdge
        if e < thumbnailBelow { return .thumbnail }
        if e < lowBelow { return .low }
        if e < mediumBelow { return .medium }
        return .full
    }

    /// Thresholds as (tier above, the edge at which it starts).
    private var steps: [(StreamTier, CGFloat)] {
        [(.low, thumbnailBelow), (.medium, lowBelow), (.full, mediumBelow)]
    }

    /// The tier to hold given the current one: a drop only when the size is
    /// clearly under the current tier's floor.
    public func stickyTier(current: StreamTier, for v: TileVisibility) -> StreamTier {
        let raw = rawTier(for: v)
        guard raw < current, raw != .paused, !v.focused else { return raw }
        // Floor of the current tier, lowered by the hysteresis factor.
        guard let floor = steps.first(where: { $0.0 == current })?.1 else { return raw }
        return v.drawnLongEdge >= floor * hysteresis ? current : raw
    }
}

/// One tile's tier over time.
public struct LODState: Equatable, Sendable {
    public private(set) var tier: StreamTier
    private var pendingDown: StreamTier?
    private var pendingSince: TimeInterval = 0

    public init(tier: StreamTier = .paused) { self.tier = tier }

    public enum Change: Equatable, Sendable {
        /// Apply these preferences; `keyframe` asks the Space for an IDR.
        case apply(StreamTier, keyframe: Bool)
    }

    /// Feed a fresh measurement at time `now`; returns a change to send, if any.
    public mutating func update(_ v: TileVisibility, now: TimeInterval,
                                policy: LODPolicy) -> Change? {
        let target = policy.stickyTier(current: tier, for: v)
        if target > tier {
            pendingDown = nil
            tier = target
            return .apply(target, keyframe: true)
        }
        if target == tier {
            pendingDown = nil
            return nil
        }
        // A step down: wait out the hold (a zoom passing through).
        if pendingDown != target {
            pendingDown = target
            pendingSince = now
            return nil
        }
        guard now - pendingSince >= policy.downgradeHold else { return nil }
        pendingDown = nil
        tier = target
        return .apply(target, keyframe: false)
    }
}

/// Totals for the HUD and the benchmark.
public struct LODCensus: Equatable, Sendable {
    public var counts: [StreamTier: Int] = [:]

    public init(_ tiers: [StreamTier]) {
        for t in tiers { counts[t, default: 0] += 1 }
    }

    /// Frames per second the Space encoders are asked for, summed.
    public var requestedFps: Int {
        counts.reduce(0) { $0 + Int($1.key.preferences.maxFps) * $1.value }
    }

    public var decoding: Int { counts.filter { $0.key.decodes }.reduce(0) { $0 + $1.value } }
}
