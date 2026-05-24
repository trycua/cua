import CoreGraphics
import CuaDriverCore
import Foundation
import ImageIO
import MCP

/// Zoom into a region of a window screenshot at full resolution.
/// Coordinates are in the same space as the (possibly resized)
/// image returned by `get_app_state`. The tool captures the window
/// at native resolution, maps the region back to original pixels,
/// crops, and returns the cropped region without further resizing.
public enum ZoomTool {
    private static let capture = WindowCapture()

    /// Maximum zoom region width in scaled-image pixels.
    private static let maxZoomWidth: Double = 500

    public static let handler = ToolHandler(
        tool: Tool(
            name: "zoom",
            description: """
                Zoom into a rectangular region of a window screenshot at
                full (native) resolution. Use this when `get_window_state`
                returned a resized image and you need to read small text,
                identify icons, or verify UI details.

                Coordinates `x1, y1, x2, y2` are in the same pixel space
                as the screenshot returned by `get_window_state` (i.e. the
                resized image if `max_image_dimension` is active). The
                maximum zoom region width is 500 px in scaled-image
                coordinates.

                The tool maps coordinates back to original (native)
                resolution by multiplying by the compression ratio z
                (original_size / scaled_size). A 500 px-wide region in
                the scaled image becomes 500 × z native pixels, so the
                returned image will be *larger* than 500 px whenever the
                unzoomed screenshot was downscaled. This is the intended
                behaviour — the zoom undoes the compression and shows
                the region at full pixel density.

                A 20% padding is automatically added on every side of
                the requested region so the target remains visible even
                if the caller's coordinates are slightly off. The
                padding is clamped to the image bounds.

                Requires `get_window_state(pid, window_id)` earlier in this
                session so the resize ratio is known.
                """,
            inputSchema: [
                "type": "object",
                "required": ["pid", "x1", "y1", "x2", "y2"],
                "properties": [
                    "pid": [
                        "type": "integer",
                        "description": "Target process ID.",
                    ],
                    "x1": [
                        "type": "number",
                        "description": "Left edge of the region (resized-image pixels).",
                    ],
                    "y1": [
                        "type": "number",
                        "description": "Top edge of the region (resized-image pixels).",
                    ],
                    "x2": [
                        "type": "number",
                        "description": "Right edge of the region (resized-image pixels).",
                    ],
                    "y2": [
                        "type": "number",
                        "description": "Bottom edge of the region (resized-image pixels).",
                    ],
                ],
                "additionalProperties": false,
            ],
            annotations: .init(
                readOnlyHint: true,
                destructiveHint: false,
                idempotentHint: false,
                openWorldHint: false
            )
        ),
        invoke: { arguments in
            guard let rawPid = arguments?["pid"]?.intValue else {
                return errorResult("Missing required integer field pid.")
            }
            guard let pid = Int32(exactly: rawPid) else {
                return errorResult(
                    "pid \(rawPid) is outside the supported Int32 range.")
            }

            guard let x1 = coerceDouble(arguments?["x1"]),
                  let y1 = coerceDouble(arguments?["y1"]),
                  let x2 = coerceDouble(arguments?["x2"]),
                  let y2 = coerceDouble(arguments?["y2"])
            else {
                return errorResult("Missing required region coordinates (x1, y1, x2, y2).")
            }

            guard x2 > x1, y2 > y1 else {
                return errorResult("Invalid region: x2 must be > x1 and y2 must be > y1.")
            }

            let regionWidth = x2 - x1
            if regionWidth > maxZoomWidth {
                return errorResult(
                    "Zoom region too wide: \(Int(regionWidth)) px > \(Int(maxZoomWidth)) px max. "
                    + "Use a narrower region (max \(Int(maxZoomWidth)) px in scaled-image coordinates)."
                )
            }

            // Scale coordinates back to original resolution if resized
            let ratio = await ImageResizeRegistry.shared.ratio(forPid: pid) ?? 1.0
            let origX1 = Int(x1 * ratio)
            let origY1 = Int(y1 * ratio)
            let origX2 = Int(x2 * ratio)
            let origY2 = Int(y2 * ratio)

            // Pad by 20% on each side so the target stays visible even
            // if the caller's coordinates are slightly off.
            let padW = Int(Double(origX2 - origX1) * 0.20)
            let padH = Int(Double(origY2 - origY1) * 0.20)
            let paddedX1 = origX1 - padW
            let paddedY1 = origY1 - padH
            let paddedX2 = origX2 + padW
            let paddedY2 = origY2 + padH

            // Capture at native resolution (no resize)
            guard let shot = try? await capture.captureFrontmostWindow(pid: pid)
            else {
                return errorResult("No capturable window for pid \(pid).")
            }

            // Decode the captured image to crop it
            guard let provider = CGDataProvider(data: shot.imageData as CFData),
                  let fullImage = CGImage(
                    jpegDataProviderSource: provider,
                    decode: nil,
                    shouldInterpolate: true,
                    intent: .defaultIntent
                  )
            else {
                return errorResult("Failed to decode captured image.")
            }

            // Clamp padded region to image bounds
            let cropX = max(0, min(paddedX1, fullImage.width - 1))
            let cropY = max(0, min(paddedY1, fullImage.height - 1))
            let cropW = min(paddedX2 - cropX, fullImage.width - cropX)
            let cropH = min(paddedY2 - cropY, fullImage.height - cropY)

            guard cropW > 0, cropH > 0 else {
                return errorResult("Crop region is empty after clamping to image bounds.")
            }

            let cropRect = CGRect(x: cropX, y: cropY, width: cropW, height: cropH)
            guard let cropped = fullImage.cropping(to: cropRect) else {
                return errorResult("Failed to crop image.")
            }

            // Encode cropped region as JPEG
            let buffer = NSMutableData()
            guard let dest = CGImageDestinationCreateWithData(
                buffer, "public.jpeg" as CFString, 1, nil)
            else {
                return errorResult("Failed to create image encoder.")
            }
            CGImageDestinationAddImage(dest, cropped, [
                kCGImageDestinationLossyCompressionQuality: 0.90
            ] as CFDictionary)
            guard CGImageDestinationFinalize(dest) else {
                return errorResult("Failed to encode cropped image.")
            }

            let base64 = (buffer as Data).base64EncodedString()

            // Store zoom context so click(from_zoom=true) can map automatically.
            await ImageResizeRegistry.shared.setZoom(
                ZoomContext(originX: cropX, originY: cropY,
                            width: cropW, height: cropH, ratio: ratio),
                forPid: pid)

            let summary = "✅ Zoomed region captured at native resolution. "
                + "To click a target in this image, use "
                + "`click(pid, x, y, from_zoom=true)` where x,y are pixel "
                + "coordinates in THIS zoomed image — the driver maps them "
                + "back automatically."

            return try CallTool.Result(
                content: [
                    .image(data: base64, mimeType: "image/jpeg", annotations: nil, _meta: nil),
                    .text(text: summary, annotations: nil, _meta: nil),
                ]
            )
        }
    )

    private static func coerceDouble(_ value: Value?) -> Double? {
        if let d = value?.doubleValue { return d }
        if let i = value?.intValue { return Double(i) }
        return nil
    }

    private static func errorResult(_ message: String) -> CallTool.Result {
        CallTool.Result(
            content: [.text(text: message, annotations: nil, _meta: nil)],
            isError: true
        )
    }
}
