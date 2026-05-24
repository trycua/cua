import CuaDriverCore
import Foundation
import MCP

/// Read-back for the persistent driver config stored at
/// `~/Library/Application Support/<app-name>/config.json`. Returns the
/// full current config in one call — callers that only want one field
/// pull it out of the structured response.
///
/// Pure read-only — no side effects, no file writes. A missing or
/// malformed config file returns defaults (same tolerance as the
/// daemon's startup load).
public enum GetConfigTool {
    public static let handler = ToolHandler(
        tool: Tool(
            name: "get_config",
            description: """
                Report the current persistent driver config. Config lives
                at `~/Library/Application Support/<app-name>/config.json`
                and survives daemon restarts, unlike session-scoped state
                (recording).

                Pure read-only. Returns defaults when the file doesn't
                exist or fails to decode — same fallback the daemon uses
                at startup. Sibling to `set_config` / `cua-driver config`.

                Current schema:

                  {
                    "schema_version": 1,
                    "capture_mode": "vision" | "ax" | "som",
                    "agent_cursor": {
                      "enabled": true,
                      "motion": {
                        "start_handle": 0.3, "end_handle": 0.3,
                        "arc_size": 0.25, "arc_flow": 0.0,
                        "spring": 0.72
                      }
                    }
                  }
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
            let config = await ConfigStore.shared.load()
            // Emit the same JSON shape that hits disk so tool consumers
            // and `cat config.json` readers see an identical payload.
            // Falls back to a text-only response if encoding fails for
            // any reason — structured content is a nicety, not a
            // contract, and a broken encoder shouldn't take down the tool.
            let encoder = ConfigStore.makeEncoder()
            let pretty: String
            if let data = try? encoder.encode(config),
                let str = String(data: data, encoding: .utf8)
            {
                pretty = str
            } else {
                pretty = "config: schema_version=\(config.schemaVersion)"
            }
            return CallTool.Result(
                content: [.text(text: "✅ \(pretty)", annotations: nil, _meta: nil)]
            )
        }
    )
}
