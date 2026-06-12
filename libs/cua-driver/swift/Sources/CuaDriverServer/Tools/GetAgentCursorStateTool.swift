import CuaDriverCore
import Foundation
import MCP

/// Read-back for the agent-cursor configuration that `set_agent_cursor_enabled`
/// and `set_agent_cursor_motion` write to. Returns the full current state in
/// one call so a caller can confirm a previous write, log the session config,
/// or render a dashboard.
public enum GetAgentCursorStateTool {
    public static let handler = ToolHandler(
        tool: Tool(
            name: "get_agent_cursor_state",
            description: """
                Report the current agent-cursor configuration: enabled flag,
                motion knobs (startHandle, endHandle, arcSize, arcFlow,
                spring), glide duration, post-click dwell, and idle-hide
                delay. Durations come back in milliseconds to match the
                setter's units. Pure read-only — no side effects.
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
            let snapshot = await MainActor.run { () -> CursorStateSnapshot in
                let c = AgentCursor.shared
                return CursorStateSnapshot(
                    enabled: c.isEnabled,
                    motion: c.defaultMotionOptions,
                    glideMs: c.glideDurationSeconds * 1000,
                    dwellMs: c.dwellAfterClickSeconds * 1000,
                    idleHideMs: c.idleHideDelay * 1000
                )
            }

            let summary =
                "cursor: enabled=\(snapshot.enabled)"
                + " startHandle=\(snapshot.motion.startHandle)"
                + " endHandle=\(snapshot.motion.endHandle)"
                + " arcSize=\(snapshot.motion.arcSize)"
                + " arcFlow=\(snapshot.motion.arcFlow)"
                + " spring=\(snapshot.motion.spring)"
                + " glideDurationMs=\(Int(snapshot.glideMs))"
                + " dwellAfterClickMs=\(Int(snapshot.dwellMs))"
                + " idleHideMs=\(Int(snapshot.idleHideMs))"

            return CallTool.Result(
                content: [.text(text: "✅ \(summary)", annotations: nil, _meta: nil)]
            )
        }
    )

    private struct CursorStateSnapshot {
        let enabled: Bool
        let motion: CursorMotionPath.Options
        let glideMs: Double
        let dwellMs: Double
        let idleHideMs: Double
    }
}
