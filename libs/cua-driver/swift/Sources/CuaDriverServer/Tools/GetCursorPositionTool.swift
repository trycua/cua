import CuaDriverCore
import Foundation
import MCP

public enum GetCursorPositionTool {
    public static let handler = ToolHandler(
        tool: Tool(
            name: "get_cursor_position",
            description:
                "Return the current mouse cursor position in screen points (origin top-left).",
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
            let pos = CursorControl.currentPosition()
            let point = CursorPoint(x: Int(pos.x), y: Int(pos.y))
            return CallTool.Result(
                content: [
                    .text(
                        text: "✅ Cursor at (\(point.x), \(point.y))",
                        annotations: nil,
                        _meta: nil
                    )
                ]
            )
        }
    )
}
