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
                image data in the requested format (default png).

                Without `window_id`, captures the full main display. With `window_id`,
                captures just that window (get the id from `list_windows`).

                Requires the Screen Recording TCC grant — call `check_permissions`
                first if unsure.
                """,
            inputSchema: [
                "type": "object",
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
                            "Optional CGWindowID / kCGWindowNumber to capture just that window.",
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
            let windowID = arguments?["window_id"]?.intValue

            do {
                let shot: Screenshot
                if let windowID {
                    shot = try await capture.captureWindow(
                        windowID: UInt32(windowID),
                        format: format,
                        quality: quality
                    )
                } else {
                    shot = try await capture.captureMainDisplay(
                        format: format,
                        quality: quality
                    )
                }
                let base64 = shot.imageData.base64EncodedString()
                let mime = format == .png ? "image/png" : "image/jpeg"
                let visibleWindows = WindowEnumerator.visibleWindows().filter { $0.layer == 0 }
                var summaryLines: [String] = [
                    "✅ Screenshot — \(shot.width)x\(shot.height) \(format.rawValue)"
                ]
                if !visibleWindows.isEmpty {
                    summaryLines.append("\nOn-screen windows:")
                    for w in visibleWindows {
                        let title = w.name.isEmpty ? "(no title)" : "\"\(w.name)\""
                        summaryLines.append("- \(w.owner) (pid \(w.pid)) \(title) [window_id: \(w.id)]")
                    }
                    summaryLines.append("→ Call get_window_state(pid, window_id) to inspect a window's UI.")
                }
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
