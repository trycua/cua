// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CoreGraphics
import Foundation

/// The single mapping between a view's coordinates and the streamed surface.
///
/// **This type exists because of a bug.** A resized stream in the sibling Spaces
/// app drew its cursor overlay at one scale and computed its click coordinate at
/// another: the overlay used the letterboxed content rect, the click used the
/// raw view bounds. The cursor sat where you pointed and the click landed
/// somewhere else, and the discrepancy only appeared once the window stopped
/// being exactly the stream's aspect ratio — so it survived every square test.
///
/// The fix is structural, not a correction: there is exactly one function that
/// converts, every consumer goes through it, and the overlay position is
/// *derived from the same call* that produces the coordinate being sent. A view
/// that wants to draw a cursor at a point must obtain that point by round-trip:
/// `point(forNormalized: normalized(for: viewPoint))`. If the two scales ever
/// diverge again, the cursor visibly lags the pointer instead of silently
/// clicking elsewhere.
public struct StreamGeometry: Equatable {
    /// Size of the decoded frame, in the stream's own pixels. This is the
    /// coordinate space RCDP normalizes against — not the host window's points,
    /// and not the `WindowDescriptor` geometry, which can differ from the
    /// session's own geometry because `max_dimension` is applied per session.
    public var surfaceSize: CGSize
    /// Bounds of the view rendering it.
    public var viewSize: CGSize

    public init(surfaceSize: CGSize, viewSize: CGSize) {
        self.surfaceSize = surfaceSize
        self.viewSize = viewSize
    }

    public var isUsable: Bool {
        surfaceSize.width > 0 && surfaceSize.height > 0 && viewSize.width > 0 && viewSize.height > 0
    }

    /// Uniform scale applied to the surface to fit the view, preserving aspect.
    public var scale: CGFloat {
        guard isUsable else { return 1 }
        return min(viewSize.width / surfaceSize.width, viewSize.height / surfaceSize.height)
    }

    /// Where the frame is actually drawn inside the view, letterboxing included.
    /// Both rendering and hit-testing use this rect — that is the whole point.
    public var contentRect: CGRect {
        guard isUsable else { return .zero }
        let size = CGSize(width: surfaceSize.width * scale, height: surfaceSize.height * scale)
        return CGRect(x: ((viewSize.width - size.width) / 2).rounded(),
                      y: ((viewSize.height - size.height) / 2).rounded(),
                      width: size.width.rounded(),
                      height: size.height.rounded())
    }

    /// Convert a point in **top-left-origin view coordinates** to the normalized
    /// `[0, 1]` surface coordinate RCDP's `interactive_input` expects.
    ///
    /// Returns `nil` for a point outside the drawn frame. A `nil` here must
    /// become "send nothing", never a clamped or sentinel coordinate — see
    /// `InputEncoder`.
    public func normalized(for viewPoint: CGPoint) -> CGPoint? {
        let rect = contentRect
        guard rect.width > 0, rect.height > 0 else { return nil }
        let x = (viewPoint.x - rect.minX) / rect.width
        let y = (viewPoint.y - rect.minY) / rect.height
        guard x.isFinite, y.isFinite, (0 ... 1).contains(x), (0 ... 1).contains(y) else { return nil }
        return CGPoint(x: x, y: y)
    }

    /// The inverse. Used to place the cursor overlay, so overlay and coordinate
    /// cannot drift apart.
    public func point(forNormalized normalized: CGPoint) -> CGPoint {
        let rect = contentRect
        return CGPoint(x: rect.minX + normalized.x * rect.width,
                       y: rect.minY + normalized.y * rect.height)
    }

    /// Surface-pixel coordinate, for the compatibility `action` path whose basis
    /// is `pixel` rather than normalized.
    public func surfacePixel(forNormalized normalized: CGPoint) -> CGPoint {
        CGPoint(x: (normalized.x * surfaceSize.width).rounded(.down),
                y: (normalized.y * surfaceSize.height).rounded(.down))
    }
}
