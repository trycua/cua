import CuaDriverCore
import Foundation
import MCP

public enum HotkeyTool {
    public static let handler = ToolHandler(
        tool: Tool(
            name: "hotkey",
            description: """
                Press a combination of keys simultaneously — e.g.
                `["cmd", "c"]` for Copy, `["cmd", "shift", "4"]` for
                screenshot selection. The combo is posted directly to
                the target pid's event queue via `CGEvent.postToPid`;
                the target does NOT need to be frontmost.

                Recognized modifiers: cmd/command, shift, option/alt,
                ctrl/control, fn. Non-modifier keys use the same
                vocabulary as `press_key` (return, tab, escape,
                up/down/left/right, space, delete, home, end, pageup,
                pagedown, f1-f12, letters, digits). Order: modifiers
                first, one non-modifier last.
                """,
            inputSchema: [
                "type": "object",
                "required": ["pid", "keys"],
                "properties": [
                    "pid": [
                        "type": "integer",
                        "description": "Target process ID.",
                    ],
                    "keys": [
                        "type": "array",
                        "items": ["type": "string"],
                        "minItems": 2,
                        "description":
                            "Modifier(s) and one non-modifier key, e.g. [\"cmd\", \"c\"].",
                    ],
                ],
                "additionalProperties": false,
            ],
            annotations: .init(
                readOnlyHint: false,
                destructiveHint: true,
                idempotentHint: false,
                openWorldHint: true
            )
        ),
        invoke: { arguments in
            guard let rawPid = arguments?["pid"]?.intValue else {
                return errorResult("Missing required integer field pid.")
            }
            guard let raw = arguments?["keys"]?.arrayValue else {
                return errorResult("Missing required array field keys.")
            }
            let keys = raw.compactMap { $0.stringValue }
            guard keys.count == raw.count, !keys.isEmpty else {
                return errorResult("keys must be a non-empty array of strings.")
            }
            guard let pid = Int32(exactly: rawPid) else {
                return errorResult(
                    "pid \(rawPid) is outside the supported Int32 range.")
            }
            do {
                try KeyboardInput.hotkey(keys, toPid: pid)
                return CallTool.Result(
                    content: [
                        .text(
                            text:
                                "✅ Pressed \(keys.joined(separator: "+")) on pid \(rawPid).",
                            annotations: nil, _meta: nil)
                    ]
                )
            } catch let error as KeyboardError {
                return errorResult(error.description)
            } catch {
                return errorResult("Unexpected error: \(error)")
            }
        }
    )

    private static func errorResult(_ message: String) -> CallTool.Result {
        CallTool.Result(
            content: [.text(text: message, annotations: nil, _meta: nil)],
            isError: true
        )
    }
}
