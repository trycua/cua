import CuaDriverCore
import Foundation
import MCP

/// Toggle the visual agent-cursor overlay. When enabled, element-
/// indexed pointer actions (`click`, `right_click`) animate a
/// floating arrow to the target's on-screen position before firing
/// the AX action — a trust signal for the user. Default: **enabled**;
/// call with `{"enabled": false}` to opt out for headless / CI runs.
public enum SetAgentCursorEnabledTool {
    public static let handler = ToolHandler(
        tool: Tool(
            name: "set_agent_cursor_enabled",
            description: """
                Toggle the visual agent-cursor overlay. When enabled, future
                pointer actions animate a floating arrow to the target's
                on-screen position before firing the AX action — purely
                visual, the AX dispatch itself is unchanged. Disabling
                removes the overlay immediately.

                Default: **enabled**. Stays on for the life of the MCP
                session / daemon; disable with `{"enabled": false}` for
                headless / CI runs where the visual isn't wanted. The
                overlay only renders when the driver has an AppKit run
                loop, which is bootstrapped by `mcp` and `serve` but not
                by one-shot CLI invocations — so this flag is a no-op in
                fresh-CLI mode.
                """,
            inputSchema: [
                "type": "object",
                "required": ["enabled"],
                "properties": [
                    "enabled": [
                        "type": "boolean",
                        "description": "True to show the overlay cursor; false to hide.",
                    ]
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
            await MainActor.run {
                AgentCursor.shared.setEnabled(enabled)
            }
            // Persist through to the on-disk config so the next daemon
            // restart boots in the same enabled/disabled state. Live
            // state already flipped above — this is purely for the
            // durability promise.
            do {
                try await ConfigStore.shared.mutate { config in
                    config.agentCursor.enabled = enabled
                }
            } catch {
                return errorResult(
                    "Agent cursor live state updated, but persisting to config failed: \(error.localizedDescription)"
                )
            }
            return CallTool.Result(
                content: [
                    .text(
                        text: enabled
                            ? "✅ Agent cursor enabled."
                            : "✅ Agent cursor disabled.",
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
