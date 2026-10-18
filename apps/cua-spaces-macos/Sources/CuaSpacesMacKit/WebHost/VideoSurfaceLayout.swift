// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CoreGraphics
import Foundation

/// What a video slot is for: a Space tile on the Spaces grid (small, a low
/// frame rate, view only) or the Space viewer (full size and rate, input).
enum VideoTier: String, Sendable {
    case tile
    case full

    /// The frame-rate cap the stream opens with (0: the Space's default, 30).
    var maxFPS: UInt32 {
        switch self {
        case .tile: return 10
        case .full: return 0
        }
    }

    /// The long-edge cap in pixels (0: the display's own size). A tile is
    /// about 230 to 400 CSS px wide, so 960 covers it at 2x.
    var maxDimension: UInt32 {
        switch self {
        case .tile: return 960
        case .full: return 0
        }
    }
}

/// One slot as the page reports it (`apps/cua-spaces-web/src/lib/stream-surface.ts`,
/// `SurfaceState`). Rects are CSS px from the web view's top left.
struct VideoSurfaceUpdate: Equatable {
    var surfaceId: String
    var spaceId: String
    var tier: VideoTier
    var interactive: Bool
    var rect: CGRect
    /// The visible part of `rect`; nil when none of it is.
    var clip: CGRect?
    var radius: CGFloat
    var occluded: Bool
    var visible: Bool

    /// Reads one entry of a `surfaces` message's `update`; nil when it is malformed.
    init?(json: [String: Any]) {
        guard let surfaceId = json["surfaceId"] as? String, !surfaceId.isEmpty,
              let spaceId = json["spaceId"] as? String, !spaceId.isEmpty,
              let rect = Self.rect(json["rect"]) else { return nil }
        self.surfaceId = surfaceId
        self.spaceId = spaceId
        tier = (json["tier"] as? String).flatMap(VideoTier.init(rawValue:)) ?? .full
        interactive = json["interactive"] as? Bool ?? false
        self.rect = rect
        clip = Self.rect(json["clip"])
        radius = Self.number(json["radius"])
        occluded = json["occluded"] as? Bool ?? false
        visible = json["visible"] as? Bool ?? true
    }

    init(surfaceId: String, spaceId: String, tier: VideoTier, interactive: Bool, rect: CGRect,
         clip: CGRect?, radius: CGFloat = 0, occluded: Bool = false, visible: Bool = true) {
        self.surfaceId = surfaceId
        self.spaceId = spaceId
        self.tier = tier
        self.interactive = interactive
        self.rect = rect
        self.clip = clip
        self.radius = radius
        self.occluded = occluded
        self.visible = visible
    }

    static func rect(_ value: Any?) -> CGRect? {
        guard let r = value as? [String: Any] else { return nil }
        let (x, y, w, h) = (number(r["x"]), number(r["y"]), number(r["width"]), number(r["height"]))
        guard x.isFinite, y.isFinite, w.isFinite, h.isFinite, w >= 0, h >= 0 else { return nil }
        return CGRect(x: x, y: y, width: w, height: h)
    }

    static func number(_ value: Any?) -> CGFloat {
        CGFloat((value as? NSNumber)?.doubleValue ?? 0)
    }
}

/// A `cuaVideo` message from the page.
enum VideoSurfaceMessage: Equatable {
    case surfaces(update: [VideoSurfaceUpdate], remove: [String])
    case focus(String?)

    init?(body: Any) {
        guard let body = body as? [String: Any], let type = body["type"] as? String else { return nil }
        switch type {
        case "surfaces":
            let update = (body["update"] as? [[String: Any]] ?? []).compactMap(VideoSurfaceUpdate.init(json:))
            let remove = (body["remove"] as? [Any] ?? []).compactMap { $0 as? String }
            self = .surfaces(update: update, remove: remove)
        case "focus":
            self = .focus(body["surfaceId"] as? String)
        default:
            return nil
        }
    }
}

/// Where a slot's native views go over the web view.
///
/// Two views per slot: a clip container at the slot's visible part (so a
/// tile half under a scroll container's edge is cut there, not drawn over
/// the page's chrome), and the video inside it at the slot's full rect
/// (rounded to the slot's radius). CSS px become view points through the
/// page zoom; a web view that is not flipped counts y from the bottom.
struct VideoSurfaceLayout: Equatable {
    /// The clip container, in the web view's coordinates.
    var container: CGRect
    /// The video, in the container's flipped (top-left) coordinates.
    var video: CGRect
    var radius: CGFloat
    /// Nothing to draw: off screen, clipped away, covered by page UI or empty.
    var hidden: Bool

    static let none = VideoSurfaceLayout(container: .zero, video: .zero, radius: 0, hidden: true)

    static func make(_ u: VideoSurfaceUpdate, zoom: CGFloat, viewHeight: CGFloat, flipped: Bool) -> VideoSurfaceLayout {
        let z = zoom > 0 && zoom.isFinite ? zoom : 1
        guard u.visible, !u.occluded, let clipCSS = u.clip?.intersection(u.rect), !clipCSS.isNull,
              clipCSS.width > 0, clipCSS.height > 0, u.rect.width > 0, u.rect.height > 0 else { return .none }
        let clip = scaled(clipCSS, z)
        let rect = scaled(u.rect, z)
        let y = flipped ? clip.minY : viewHeight - clip.maxY
        return VideoSurfaceLayout(
            container: CGRect(x: clip.minX, y: y, width: clip.width, height: clip.height),
            video: rect.offsetBy(dx: -clip.minX, dy: -clip.minY),
            radius: max(0, u.radius * z),
            hidden: false)
    }

    private static func scaled(_ r: CGRect, _ z: CGFloat) -> CGRect {
        CGRect(x: r.minX * z, y: r.minY * z, width: r.width * z, height: r.height * z)
    }
}

/// How a slot's stream is going, as the page hears it (`video.surface`).
enum VideoSurfacePhase: Equatable {
    case connecting
    case live
    /// `opening`: no stream could be opened at all (`streamProvider`
    /// threw), so the page says why once, as the SwiftUI detail's banner.
    case failed(String, opening: Bool = false)

    var word: String {
        switch self {
        case .connecting: return "connecting"
        case .live: return "live"
        case .failed: return "failed"
        }
    }

    var payload: [String: Any] {
        if case let .failed(reason, opening) = self {
            return opening ? ["state": word, "reason": reason, "opening": true] : ["state": word, "reason": reason]
        }
        return ["state": word]
    }
}
