import CuaDriverCore
import Foundation
import MCP

public enum CheckPermissionsTool {
    public static let handler = ToolHandler(
        tool: Tool(
            name: "check_permissions",
            description: """
                Report TCC permission status for Accessibility and Screen Recording.
                By default also raises the system permission dialogs for any missing
                grants — Apple's request APIs are no-ops when the grant is already
                active, so this is safe to call repeatedly. Pass {"prompt": false}
                for a purely read-only status check.
                """,
            inputSchema: [
                "type": "object",
                "properties": [
                    "prompt": [
                        "type": "boolean",
                        "description":
                            "Raise the system permission prompts for missing grants. Default true.",
                    ]
                ],
                "additionalProperties": false,
            ],
            annotations: .init(
                // Not readOnly because the default path may raise a modal dialog.
                readOnlyHint: false,
                destructiveHint: false,
                idempotentHint: true,
                openWorldHint: false
            )
        ),
        invoke: { arguments in
            // Default to prompting. The point of `check_permissions` is to
            // surface and resolve missing grants — a read-only status check
            // that leaves the user hunting for `CuaDriver.app` in System
            // Settings is the wrong default. Apple's request APIs no-op
            // when the grant is already active, so prompting is safe.
            let shouldPrompt = arguments?["prompt"]?.boolValue ?? true
            if shouldPrompt {
                _ = Permissions.requestAccessibility()
                _ = Permissions.requestScreenRecording()
            }
            let status = await Permissions.currentStatus()
            let accessibilityPrefix = status.accessibility ? "✅" : "❌"
            let screenRecordingPrefix = status.screenRecording ? "✅" : "❌"
            let summary =
                """
                \(accessibilityPrefix) Accessibility: \(status.accessibility ? "granted" : "NOT granted").
                \(screenRecordingPrefix) Screen Recording: \(status.screenRecording ? "granted" : "NOT granted").
                """
            return CallTool.Result(
                content: [.text(text: summary, annotations: nil, _meta: nil)]
            )
        }
    )
}
