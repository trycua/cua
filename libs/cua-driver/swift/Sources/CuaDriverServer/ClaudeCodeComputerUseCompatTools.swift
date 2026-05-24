import AppKit
import CuaDriverCore
import Foundation
import MCP

private struct CompatWindowContext: Sendable {
    let window: WindowInfo
    let scaleFactor: Double
}

private actor ClaudeCodeComputerUseCompatSession {
    static let shared = ClaudeCodeComputerUseCompatSession()

    private var activeWindow: CompatWindowContext?

    func setActiveWindow(_ context: CompatWindowContext?) {
        activeWindow = context
    }

    func currentActiveWindow() -> CompatWindowContext? {
        activeWindow
    }
}

public enum ClaudeCodeComputerUseCompatTools {
    private static let capture = WindowCapture()

    public static let screenshot = ToolHandler(
        tool: Tool(
            name: "screenshot",
            description: """
                Capture a target window and return a JPEG image. Coordinates accepted
                by CuaDriver's pixel tools are pixels in this window screenshot's
                coordinate space.

                This is the compatibility anchor for Claude Code vision flows:
                CuaDriver remains window-scoped, and all other tools are the
                normal CuaDriver tools.
                """,
            inputSchema: [
                "type": "object",
                "required": ["pid", "window_id"],
                "properties": [
                    "pid": [
                        "type": "integer",
                        "description": "Target process ID from `list_windows` or `launch_app`.",
                    ],
                    "window_id": [
                        "type": "integer",
                        "description": "Target CGWindowID from `list_windows` or `launch_app`.",
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
            do {
                guard let rawPid = arguments?["pid"]?.intValue else {
                    return errorResult("Missing required integer field `pid`.")
                }
                guard let pid = Int32(exactly: rawPid) else {
                    return errorResult("pid \(rawPid) is outside the supported Int32 range.")
                }
                guard let rawWindowID = arguments?["window_id"]?.intValue else {
                    return errorResult("Missing required integer field `window_id`.")
                }
                guard let windowID = UInt32(exactly: rawWindowID) else {
                    return errorResult(
                        "window_id \(rawWindowID) is outside the supported UInt32 range.")
                }
                guard let context = compatWindowContext(
                    forPid: pid,
                    windowID: windowID
                ) else {
                    return errorResult(
                        "No visible layer-0 window \(rawWindowID) found for pid \(rawPid). Use `list_windows` to choose an on-screen target window."
                    )
                }
                let shot = try await capture.captureWindow(
                    windowID: windowID,
                    format: .jpeg,
                    quality: 85
                )
                await ClaudeCodeComputerUseCompatSession.shared.setActiveWindow(
                    CompatWindowContext(
                        window: context.window,
                        scaleFactor: shot.scaleFactor
                    )
                )
                let base64 = shot.imageData.base64EncodedString()
                return CallTool.Result(content: [
                    .image(data: base64, mimeType: "image/jpeg", annotations: nil, _meta: nil),
                    .text(
                        text: "Captured window screenshot \(shot.width)x\(shot.height) for \(context.window.owner) [pid: \(context.window.pid), window_id: \(context.window.id)]. Use CuaDriver pixel tools with this window-local coordinate space.",
                        annotations: nil,
                        _meta: nil
                    ),
                ])
            } catch CaptureError.permissionDenied {
                return errorResult(
                    "Screen Recording permission is not granted for CuaDriver.")
            } catch {
                return errorResult("Screenshot failed: \(error)")
            }
        }
    )

    public static let all: [ToolHandler] = [
        screenshot,
    ]
}

extension ToolRegistry {
    public static let claudeCodeComputerUseCompat: ToolRegistry = {
        let shimNames = Set(ClaudeCodeComputerUseCompatTools.all.map(\.tool.name))
        let nativeHandlers = ToolRegistry.default.handlers.values
            .filter { !shimNames.contains($0.tool.name) }
        return ToolRegistry(
            handlers: Array(nativeHandlers) + ClaudeCodeComputerUseCompatTools.all
        )
    }()
}

private func compatWindowContext(
    forPid pid: Int32,
    windowID: UInt32
) -> CompatWindowContext? {
    guard let window = WindowEnumerator.visibleWindows()
        .first(where: {
            $0.pid == pid
                && UInt32(exactly: $0.id) == windowID
                && $0.layer == 0
                && $0.isOnScreen
                && $0.bounds.width > 1
                && $0.bounds.height > 1
        })
    else {
        return nil
    }
    return CompatWindowContext(
        window: window,
        scaleFactor: defaultScaleFactor()
    )
}

private func defaultScaleFactor() -> Double {
    ScreenInfo.mainScreenSize()?.scaleFactor ?? 1.0
}

private func errorResult(_ text: String) -> CallTool.Result {
    CallTool.Result(
        content: [.text(text: text, annotations: nil, _meta: nil)],
        isError: true
    )
}
