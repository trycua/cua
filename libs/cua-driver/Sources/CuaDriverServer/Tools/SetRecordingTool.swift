import CuaDriverCore
import Foundation
import MCP

/// Toggle the trajectory recorder. When enabled, every subsequent
/// action-tool invocation writes a numbered turn folder containing
/// (post-action) AX state, a per-window screenshot, the tool call
/// itself, and a click-marker overlay for click-family actions.
///
/// Session-scoped: state lives in memory, resets to disabled on
/// daemon restart. Turn numbering restarts at 1 every time recording
/// is (re-)enabled with a target directory.
public enum SetRecordingTool {
    public static let handler = ToolHandler(
        tool: Tool(
            name: "set_recording",
            description: """
                Toggle trajectory recording. When enabled, every
                subsequent action-tool invocation (click, right_click,
                scroll, type_text, type_text_chars, press_key, hotkey,
                set_value) writes a turn folder under `output_dir`:

                - `app_state.json` — post-action AX snapshot for the
                  target pid (same shape as `get_window_state`).
                - `screenshot.png` — post-action per-window screenshot
                  of the target's frontmost on-screen window.
                - `action.json` — tool name, full input arguments,
                  result summary, pid, click point (when applicable),
                  ISO-8601 timestamp.
                - `click.png` — for click-family actions only,
                  `screenshot.png` with a red dot drawn at the click
                  point.

                Turn folders are named `turn-00001/`, `turn-00002/`,
                etc. Turn numbering restarts at 1 each time recording
                is (re-)enabled.

                Required when `enabled=true`: `output_dir`. Expands `~`
                and creates the directory (and intermediates) if
                missing. Ignored when `enabled=false`.

                State persists for the life of the daemon / MCP
                session; a restart resets to disabled with no
                on-disk state.
                """,
            inputSchema: [
                "type": "object",
                "required": ["enabled"],
                "properties": [
                    "enabled": [
                        "type": "boolean",
                        "description":
                            "True to start recording subsequent action tool calls; false to stop.",
                    ],
                    "output_dir": [
                        "type": "string",
                        "description":
                            "Absolute or ~-rooted directory where turn folders are written. Required when enabled=true.",
                    ],
                    "video_experimental": [
                        "type": "boolean",
                        "description":
                            "Experimental: also capture the main display to `<output_dir>/recording.mp4` via SCStream (H.264, 30fps, no audio, no cursor). Capture-only — no zoom / post-process. Off by default. Ignored when enabled=false.",
                    ],
                ],
                "additionalProperties": false,
            ],
            annotations: .init(
                readOnlyHint: false,
                destructiveHint: false,
                idempotentHint: true,
                openWorldHint: false
            )
        ),
        invoke: { arguments in
            guard let enabled = arguments?["enabled"]?.boolValue else {
                return errorResult("Missing required boolean field `enabled`.")
            }
            if enabled {
                guard let rawDir = arguments?["output_dir"]?.stringValue,
                      !rawDir.isEmpty
                else {
                    return errorResult(
                        "`output_dir` is required when enabling recording.")
                }
                let expanded = (rawDir as NSString).expandingTildeInPath
                let url = URL(fileURLWithPath: expanded).standardizedFileURL
                let videoExperimental =
                    arguments?["video_experimental"]?.boolValue ?? false
                do {
                    try await RecordingSession.shared.configure(
                        enabled: true,
                        outputDir: url,
                        videoExperimental: videoExperimental
                    )
                } catch {
                    return errorResult(
                        "Failed to enable recording: \(error.localizedDescription)")
                }
                let videoSuffix =
                    videoExperimental
                    ? " (video: \(url.appendingPathComponent("recording.mp4").path))"
                    : ""
                return CallTool.Result(
                    content: [
                        .text(
                            text: "✅ Recording enabled -> \(url.path)\(videoSuffix)",
                            annotations: nil,
                            _meta: nil
                        )
                    ]
                )
            }
            do {
                try await RecordingSession.shared.configure(
                    enabled: false, outputDir: nil, videoExperimental: false
                )
            } catch {
                return errorResult(
                    "Failed to disable recording: \(error.localizedDescription)")
            }
            let renderedSuffix: String
            if let rendered = await RecordingSession.shared.lastAutoRenderURL {
                renderedSuffix = " Rendered: \(rendered.path)"
            } else {
                renderedSuffix = ""
            }
            return CallTool.Result(
                content: [
                    .text(
                        text: "✅ Recording disabled.\(renderedSuffix)",
                        annotations: nil,
                        _meta: nil
                    )
                ]
            )
        }
    )

    private static func errorResult(_ message: String) -> CallTool.Result {
        CallTool.Result(
            content: [.text(text: message, annotations: nil, _meta: nil)],
            isError: true
        )
    }
}
