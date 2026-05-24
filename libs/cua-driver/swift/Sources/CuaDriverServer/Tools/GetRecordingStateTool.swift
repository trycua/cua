import CuaDriverCore
import Foundation
import MCP

/// Read-back for the trajectory recorder. Returns whether recording is
/// currently enabled, the output directory the next turn will be written
/// under (when enabled), and the 1-based counter for the next turn. Pure
/// read-only — no side effects.
///
/// Primary caller is the `cua-driver recording status` CLI subcommand,
/// which formats these fields for human consumption. Also callable from
/// MCP / `cua-driver get_recording_state` for programmatic checks.
public enum GetRecordingStateTool {
    public static let handler = ToolHandler(
        tool: Tool(
            name: "get_recording_state",
            description: """
                Report the current trajectory recorder state: whether
                recording is enabled, the output directory (when
                enabled), and the 1-based counter for the next turn
                folder that will be written. Counter increments on every
                recorded action tool call and resets to 1 each time
                recording is (re-)enabled.

                Pure read-only. Typical use: `cua-driver recording
                status` surfaces these fields for human consumption;
                programmatic callers use it to confirm recording is
                armed before kicking off a session.
                """,
            inputSchema: [
                "type": "object",
                "properties": [:],
                "additionalProperties": false,
            ],
            annotations: .init(
                readOnlyHint: true,
                destructiveHint: false,
                idempotentHint: true,
                openWorldHint: false
            )
        ),
        invoke: { _ in
            let state = await RecordingSession.shared.currentState()
            let dirPath = state.outputDirectory?.path
            let summary: String
            if state.enabled, let dirPath {
                summary =
                    "recording: enabled output_dir=\(dirPath) "
                    + "next_turn=\(state.nextTurn)"
            } else {
                summary = "recording: disabled"
            }

            var structured: [String: Value] = [
                "enabled": .bool(state.enabled),
                "next_turn": .int(state.nextTurn),
            ]
            if let dirPath {
                structured["output_dir"] = .string(dirPath)
            }

            return CallTool.Result(
                content: [.text(text: "✅ \(summary)", annotations: nil, _meta: nil)]
            )
        }
    )
}
