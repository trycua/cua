import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

/// Render a red crosshair on a freshly-captured window screenshot and
/// save it to disk. Used by `click` / `right_click`'s `debug_image_out`
/// option so the caller can visually verify the tool received the
/// coordinate it intended.
///
/// The crosshair is drawn in the **same coordinate space** the pixel-
/// click path consumes — window-local PNG pixels at the currently-
/// configured `max_image_dimension`. If the client drew its own
/// "intent" crosshair on the PNG from `get_window_state` and calls
/// click with `debug_image_out` pointing at a sibling file, both
/// crosshairs should land on the same pixel. A mismatch surfaces a
/// coord-space bug immediately.
///
/// Intentionally re-captures the window (rather than reusing whatever
/// image was returned from the last `get_window_state`) so the
/// snapshot reflects the state the tool will actually click against —
/// any drift between snapshot and dispatch is visible in the saved
/// image.
public enum DebugCrosshair {
    /// Capture `windowID` at `maxImageDimension`, overlay a red
    /// crosshair + coord label at `point`, save as PNG to `path`.
    ///
    /// Errors from capture (window closed, permissions denied) or
    /// encoding propagate up. File-write errors do too — the caller
    /// asked for a specific path, so failing loudly beats silently
    /// skipping the debug artifact.
    public static func writeCrosshair(
        capture: WindowCapture = .init(),
        windowID: UInt32,
        point: CGPoint,
        maxImageDimension: Int,
        path: String
    ) async throws {
        let shot = try await capture.captureWindow(
            windowID: windowID,
            format: .png,
            quality: 100,
            maxImageDimension: maxImageDimension
        )

        guard
            let source = CGImageSourceCreateWithData(
                shot.imageData as CFData, nil
            ),
            let base = CGImageSourceCreateImageAtIndex(source, 0, nil)
        else {
            throw CrosshairError.decodeFailed
        }

        let w = base.width
        let h = base.height
        guard
            let ctx = CGContext(
                data: nil,
                width: w,
                height: h,
                bitsPerComponent: 8,
                bytesPerRow: 0,
                space: CGColorSpaceCreateDeviceRGB(),
                bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue
                    | CGBitmapInfo.byteOrder32Little.rawValue
            )
        else {
            throw CrosshairError.contextFailed
        }

        // Draw the base window screenshot into the context. The
        // crosshair is layered on top afterwards.
        ctx.draw(base, in: CGRect(x: 0, y: 0, width: w, height: h))

        // Crosshair geometry — scale slightly with image size so it
        // reads at both 460-wide Calculator shots and 1568-wide
        // Chrome shots.
        let ringRadius = max(CGFloat(6), CGFloat(w) / 80.0)
        let armLength = max(CGFloat(12), CGFloat(w) / 40.0)
        let lineWidth = max(CGFloat(1.5), CGFloat(w) / 400.0)

        // Flip y: the click tool accepts top-left-origin pixel
        // coords, but CoreGraphics' CGContext on an RGB bitmap draws
        // bottom-left-origin. Reflect the y we were handed into the
        // bitmap's own space so `point.x, point.y` lands exactly
        // where the pixel click would.
        let cx = point.x
        let cy = CGFloat(h) - point.y

        ctx.setStrokeColor(CGColor(red: 1, green: 0.1, blue: 0.1, alpha: 0.95))
        ctx.setFillColor(CGColor(red: 1, green: 0.1, blue: 0.1, alpha: 0.95))
        ctx.setLineWidth(lineWidth)

        // Outer ring — hollow circle centered on the click point.
        ctx.strokeEllipse(
            in: CGRect(
                x: cx - ringRadius, y: cy - ringRadius,
                width: ringRadius * 2, height: ringRadius * 2
            )
        )

        // Cross arms — horizontal + vertical lines through the ring.
        ctx.move(to: CGPoint(x: cx - armLength, y: cy))
        ctx.addLine(to: CGPoint(x: cx + armLength, y: cy))
        ctx.move(to: CGPoint(x: cx, y: cy - armLength))
        ctx.addLine(to: CGPoint(x: cx, y: cy + armLength))
        ctx.strokePath()

        // Center dot — small filled marker so the exact pixel is
        // unambiguous even when the ring + arms render thinly.
        let dotRadius = max(CGFloat(1.5), lineWidth * 1.5)
        ctx.fillEllipse(
            in: CGRect(
                x: cx - dotRadius, y: cy - dotRadius,
                width: dotRadius * 2, height: dotRadius * 2
            )
        )

        guard let composited = ctx.makeImage() else {
            throw CrosshairError.renderFailed
        }

        // Encode back to PNG and write to the requested path.
        let outBuffer = NSMutableData()
        let pngType = UTType.png.identifier as CFString
        guard
            let dest = CGImageDestinationCreateWithData(
                outBuffer, pngType, 1, nil
            )
        else {
            throw CrosshairError.encodeFailed
        }
        CGImageDestinationAddImage(dest, composited, nil)
        guard CGImageDestinationFinalize(dest) else {
            throw CrosshairError.encodeFailed
        }

        let url = URL(fileURLWithPath: (path as NSString).expandingTildeInPath)
        try (outBuffer as Data).write(to: url, options: .atomic)
    }
}

public enum CrosshairError: Error, CustomStringConvertible {
    case decodeFailed
    case contextFailed
    case renderFailed
    case encodeFailed

    public var description: String {
        switch self {
        case .decodeFailed: return "debug crosshair: failed to decode captured PNG"
        case .contextFailed: return "debug crosshair: failed to create CGContext"
        case .renderFailed: return "debug crosshair: failed to composite image"
        case .encodeFailed: return "debug crosshair: failed to encode PNG"
        }
    }
}
