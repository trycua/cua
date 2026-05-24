import AppKit
import CoreGraphics
import Foundation
import ImageIO
import os

/// Render a "screenshot with a red dot at the click point" PNG.
/// Split out of `RecordingSession` purely to keep that file short —
/// no state, no async, no actor isolation; just pure image I/O.
enum ClickMarkerRenderer {
    private static let log = Logger(
        subsystem: "com.trycua.driver",
        category: "ClickMarkerRenderer"
    )

    /// Write `click.png` to `destination`, painting a visible marker at
    /// the given screen-absolute click point on top of `baseImageData`
    /// (an already-encoded PNG of the captured window). Returns
    /// silently on any failure — callers always want best-effort.
    static func writeMarker(
        baseImageData: Data,
        scaleFactor: Double,
        clickPointInPoints: CGPoint,
        destination: URL
    ) {
        guard
            let source = CGImageSourceCreateWithData(baseImageData as CFData, nil),
            let image = CGImageSourceCreateImageAtIndex(source, 0, nil)
        else {
            log.error("decode screenshot for click.png failed")
            return
        }
        let width = image.width
        let height = image.height

        guard
            let bitmap = NSBitmapImageRep(
                bitmapDataPlanes: nil,
                pixelsWide: width,
                pixelsHigh: height,
                bitsPerSample: 8,
                samplesPerPixel: 4,
                hasAlpha: true,
                isPlanar: false,
                colorSpaceName: .deviceRGB,
                bytesPerRow: 0,
                bitsPerPixel: 32
            )
        else {
            log.error("allocate NSBitmapImageRep for click.png failed")
            return
        }

        guard
            let pixelPointTopLeft = windowLocalPixelPoint(
                clickInScreenPoints: clickPointInPoints,
                scaleFactor: scaleFactor,
                imageWidth: width,
                imageHeight: height
            )
        else {
            log.error("click point outside window bounds; skipping click.png")
            return
        }

        NSGraphicsContext.saveGraphicsState()
        defer { NSGraphicsContext.restoreGraphicsState() }
        guard let ctx = NSGraphicsContext(bitmapImageRep: bitmap) else {
            log.error("create NSGraphicsContext for click.png failed")
            return
        }
        NSGraphicsContext.current = ctx
        let cg = ctx.cgContext

        // CGContext uses bottom-left origin. Drawing the image into
        // a full-size rect renders it right-side-up (CG matches the
        // PNG's native orientation because it doesn't Y-flip raster
        // CGImages). We convert the click point's Y from top-left
        // (PNG-native) to bottom-left for the dot's placement.
        cg.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
        let dotY = Double(height) - pixelPointTopLeft.y

        let outerDiameter: CGFloat = 20 * CGFloat(scaleFactor)
        let innerDiameter: CGFloat = outerDiameter * 0.6
        let outerRect = CGRect(
            x: pixelPointTopLeft.x - outerDiameter / 2,
            y: dotY - outerDiameter / 2,
            width: outerDiameter,
            height: outerDiameter
        )
        let innerRect = outerRect.insetBy(
            dx: (outerDiameter - innerDiameter) / 2,
            dy: (outerDiameter - innerDiameter) / 2
        )
        // Red outer fill, white ring, red core — three-layer layout
        // stays visible against arbitrary backgrounds (light / dark /
        // busy UIs).
        cg.setFillColor(CGColor(red: 1.0, green: 0.15, blue: 0.15, alpha: 0.95))
        cg.fillEllipse(in: outerRect)
        cg.setFillColor(CGColor(red: 1.0, green: 1.0, blue: 1.0, alpha: 1.0))
        cg.fillEllipse(in: innerRect)
        cg.setFillColor(CGColor(red: 1.0, green: 0.15, blue: 0.15, alpha: 1.0))
        cg.fillEllipse(
            in: innerRect.insetBy(
                dx: innerDiameter * 0.3, dy: innerDiameter * 0.3
            )
        )

        guard let pngData = bitmap.representation(using: .png, properties: [:]) else {
            log.error("PNG-encode click.png failed")
            return
        }
        do {
            try pngData.write(to: destination, options: .atomic)
        } catch {
            log.error(
                "write click.png failed: \(error.localizedDescription, privacy: .public)"
            )
        }
    }

    /// Convert a screen-absolute click point into the screenshot's pixel
    /// coordinate space. Returns nil when no visible window for any pid
    /// contains the point or when the window's point-size doesn't match
    /// the captured image (meaning the point targets a different window
    /// than the one in the screenshot, so drawing a dot on this image
    /// would be misleading).
    ///
    /// The screenshot pipeline doesn't stash the window's screen
    /// origin, so we re-derive it: walk visible windows, find the one
    /// whose point-size matches the image's point-size AND whose
    /// bounds contain the click point, and treat that as "the window
    /// we captured."
    private static func windowLocalPixelPoint(
        clickInScreenPoints: CGPoint,
        scaleFactor: Double,
        imageWidth: Int,
        imageHeight: Int
    ) -> CGPoint? {
        let expectedPointWidth = Double(imageWidth) / scaleFactor
        let expectedPointHeight = Double(imageHeight) / scaleFactor
        for window in WindowEnumerator.visibleWindows() {
            // Allow ≤2 pt drift — the capture path uses
            // `max(1, Int(w * s))` rounding.
            let widthMatches = abs(window.bounds.width - expectedPointWidth) < 2
            let heightMatches = abs(window.bounds.height - expectedPointHeight) < 2
            if !widthMatches || !heightMatches { continue }
            let localPoint = CGPoint(
                x: clickInScreenPoints.x - window.bounds.x,
                y: clickInScreenPoints.y - window.bounds.y
            )
            if localPoint.x < 0 || localPoint.y < 0 { continue }
            if localPoint.x > window.bounds.width { continue }
            if localPoint.y > window.bounds.height { continue }
            // Top-left origin PNG coords — matches the bitmap the
            // caller is drawing into.
            return CGPoint(
                x: localPoint.x * scaleFactor,
                y: localPoint.y * scaleFactor
            )
        }
        return nil
    }
}
