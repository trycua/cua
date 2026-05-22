import CuaDriverCore
import Foundation
import MCP

public enum ScreenshotTool {
    // Single shared actor — captures serialize through it.
    private static let capture = WindowCapture()

    public static let handler = ToolHandler(
        tool: Tool(
            name: "screenshot",
            description: """
                Capture a screenshot of a single window. Returns base64-encoded
                image data in the requested format (default png).


                                `window_id` is required. Get window ids from `list_windows`.

                Requires the Screen Recording TCC grant — call `check_permissions`
                first if unsure.

                On macOS builds where pixel capture refuses a specific window,
                try a different `window_id` or switch to `capture_mode: ax` for
                `get_window_state` (the element-indexed flow doesn't need pixels).
                """,
            inputSchema: [
                "type": "object",
                "required": ["window_id"],
                "properties": [
                    "format": [
                        "type": "string",
                        "enum": ["png", "jpeg"],
                        "description": "Image format. Default: png.",
                    ],
                    "quality": [
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 95,
                        "description": "JPEG quality 1-95; ignored for png.",
                    ],
                    "window_id": [
                        "type": "integer",
                        "description":
                            "Required CGWindowID / kCGWindowNumber to capture.",
                    ],
                ],
                "additionalProperties": false,
            ],
            annotations: .init(
                readOnlyHint: true,
                destructiveHint: false,
                idempotentHint: false,  // a fresh pixel grab every call
                openWorldHint: false
            )
        ),
        invoke: { arguments in
            let format =
                ImageFormat(rawValue: arguments?["format"]?.stringValue ?? "png") ?? .png
            let quality = arguments?["quality"]?.intValue ?? 95
            guard let rawWindowID = arguments?["window_id"]?.intValue else {
                return CallTool.Result(
                    content: [
                        .text(
                            text: "Missing required `window_id`. Use `list_windows` first, then call `screenshot` for one window.",
                            annotations: nil,
                            _meta: nil
                        )
                    ],
                    isError: true
                )
            }
            guard let windowID = UInt32(exactly: rawWindowID) else {
                return CallTool.Result(
                    content: [
                        .text(
                            text: "Invalid `window_id` \(rawWindowID). Use `list_windows` first, then pass a valid UInt32 window id.",
                            annotations: nil,
                            _meta: nil
                        )
                    ],
                    isError: true
                )
            }

            do {
                let shot = try await capture.captureWindow(
                    windowID: windowID,
                    format: format,
                    quality: quality
                )
                let base64 = shot.imageData.base64EncodedString()
                let mime = format == .png ? "image/png" : "image/jpeg"
                var summaryLines: [String] = [
                    "✅ Window screenshot — \(shot.width)x\(shot.height) \(format.rawValue) [window_id: \(rawWindowID)]"
                ]
                let summary = summaryLines.joined(separator: "\n")
                return CallTool.Result(
                    content: [
                        .image(data: base64, mimeType: mime, annotations: nil, _meta: nil),
                        .text(text: summary, annotations: nil, _meta: nil),
                    ]
                )
            } catch CaptureError.permissionDenied {
                return CallTool.Result(
                    content: [
                        .text(
                            text: """
                                Screen Recording permission not granted. Call \
                                `check_permissions` with {"prompt": true} to request it, \
                                then allow cua-driver in System Settings → Privacy & \
                                Security → Screen Recording.
                                """,
                            annotations: nil,
                            _meta: nil
                        )
                    ],
                    isError: true
                )
            } catch CaptureError.streamingFailed(let msg) {
                // Pixel capture refused this specific window. There's nothing
                // else to do at this layer; surface an actionable hint pointing
                // at AX-only `get_window_state` or trying a different window.
                return CallTool.Result(
                    content: [
                        .text(
                            text: """
                                Pixel capture refused this window: \(msg)

                                Some macOS builds refuse specific windows even when \
                                Screen Recording is granted.

                                Workarounds:
                                  • Try a different `window_id` on the same app — \
                                often only one window is affected.
                                  • For element-indexed clicks, switch to AX-only: \
                                `cua-driver config set capture_mode ax` and use \
                                `get_window_state` (no screenshot, AX tree only).
                                  • Re-snapshot a moment later — the failure is \
                                sometimes transient.
                                """,
                            annotations: nil,
                            _meta: nil
                        )
                    ],
                    isError: true
                )
            } catch {
                return CallTool.Result(
                    content: [
                        .text(
                            text: "Screenshot failed: \(error)",
                            annotations: nil,
                            _meta: nil
                        )
                    ],
                    isError: true
                )
            }
        }
    )

}
