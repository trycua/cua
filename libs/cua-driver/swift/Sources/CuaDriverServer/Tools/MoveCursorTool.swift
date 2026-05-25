import CoreGraphics
import CuaDriverCore
import Foundation
import MCP

public enum MoveCursorTool {
    public static let handler = ToolHandler(
        tool: Tool(
            name: "move_cursor",
            description: """
                Instantly move the mouse cursor to (x, y) in screen points.
                Uses CGWarpMouseCursorPosition — no drag, no click, no CGEvent.
                """,
            inputSchema: [
                "type": "object",
                "required": ["x", "y"],
                "properties": [
                    "x": ["type": "integer", "description": "X in screen points."],
                    "y": ["type": "integer", "description": "Y in screen points."],
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
            guard
                let x = arguments?["x"]?.intValue,
                let y = arguments?["y"]?.intValue
            else {
                return CallTool.Result(
                    content: [
                        .text(
                            text: "Missing required integer fields x and y.",
                            annotations: nil,
                            _meta: nil
                        )
                    ],
                    isError: true
                )
            }

            CursorControl.move(to: CGPoint(x: x, y: y))
            let point = CursorPoint(x: x, y: y)
            return CallTool.Result(
                content: [
                    .text(
                        text: "✅ Moved cursor to (\(x), \(y)).",
                        annotations: nil,
                        _meta: nil
                    )
                ]
            )
        }
    )
}
