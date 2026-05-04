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
                Capture a screenshot using ScreenCaptureKit. Returns base64-encoded
                image data for a single window in the requested format (default png).


                                `window_id` is required. Get window ids from `list_windows`.

                Requires the Screen Recording TCC grant — call `check_permissions`
                first if unsure.
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
