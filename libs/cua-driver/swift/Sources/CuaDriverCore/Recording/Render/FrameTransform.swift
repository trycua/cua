// Per-frame zoom transform. Uses CoreImage's `.cropped(to:)` +
// `.transformed(by:)` — good enough for v1, no Metal. The sampler
// (`ZoomRegionGenerator.sampleCurve`) hands us scale + focus per frame;
// we derive a crop rect centered on focus at `1/scale` of frame size,
// then Lanczos-scale the crop back up to the full frame.
//
// Coordinate notes: `focusX/Y` are in screen-points (top-left origin,
// y-down), and video frames are in display-native pixels. The caller
// converts focus from points to pixels *before* calling here (see
// `RecordingRenderer.renderFrame`) and passes a `pointsToPixels`
// factor so this file does a single clean pixel-space crop. CoreImage
// itself is bottom-left origin — we flip Y inside `transformedFrame`.

import CoreImage
import CoreGraphics
import Foundation

public enum FrameTransform {
    /// Apply a zoom transform to `input` and return a CIImage sized at
    /// `frameSize` in pixels. `scale == 1.0` is a fast pass-through.
    ///
    /// `focusX` / `focusY` are in pixel-space (already converted from
    /// cursor-space points by the caller). Top-left origin — the
    /// Cocoa/CGImage convention — with Y flipped to CoreImage's
    /// bottom-left origin inside the crop math.
    public static func transformedFrame(
        _ input: CIImage,
        scale: Double,
        focusX: Double,
        focusY: Double,
        frameSize: CGSize
    ) -> CIImage {
        // Fast path: no zoom requested — skip the crop/scale dance so
        // the renderer still does one CIContext.render per frame but
        // skips unnecessary resampling.
        if abs(scale - 1.0) < 1e-6 {
            return input
        }

        let safeScale = max(scale, 1.0)
        let cropW = frameSize.width / CGFloat(safeScale)
        let cropH = frameSize.height / CGFloat(safeScale)

        // Clamp the crop rect inside the frame: if focus is near an edge,
        // slide the rect in so we don't produce a black-padded result.
        // Top-left origin first, then flip Y for CoreImage below.
        let halfW = cropW / 2
        let halfH = cropH / 2
        let focusXC = CGFloat(focusX)
        let focusYC = CGFloat(focusY)
        let cropXTopLeft = clamp(focusXC - halfW, min: 0, max: frameSize.width - cropW)
        let cropYTopLeft = clamp(focusYC - halfH, min: 0, max: frameSize.height - cropH)

        // CoreImage's origin is bottom-left. Flip the Y: CI's cropY is
        // measured from the bottom, so `frameHeight - cropYTopLeft - cropH`.
        let cropYCI = frameSize.height - cropYTopLeft - cropH

        let cropRect = CGRect(
            x: cropXTopLeft,
            y: cropYCI,
            width: cropW,
            height: cropH
        )
        let cropped = input.cropped(to: cropRect)

        // After `cropped(to:)`, the extent still reflects the crop's
        // position within the original image. Translate so the crop sits
        // at origin (0, 0), then uniform-scale back up to frame size.
        let translated = cropped.transformed(
            by: CGAffineTransform(translationX: -cropRect.origin.x, y: -cropRect.origin.y)
        )
        return translated.transformed(
            by: CGAffineTransform(scaleX: CGFloat(safeScale), y: CGFloat(safeScale))
        )
    }

    /// Clamp `v` to `[min, max]`. Pulled out as a plain free function so
    /// call sites read linearly; the stdlib spells `max(lo, min(hi, v))`.
    @inlinable
    static func clamp(_ v: CGFloat, min lo: CGFloat, max hi: CGFloat) -> CGFloat {
        // If the clamp range is degenerate (hi < lo, e.g. crop larger than
        // frame for scale < 1) fall back to the frame origin — the caller
        // has already pinned `safeScale >= 1.0` so this is defensive.
        if hi < lo { return lo }
        if v < lo { return lo }
        if v > hi { return hi }
        return v
    }
}
