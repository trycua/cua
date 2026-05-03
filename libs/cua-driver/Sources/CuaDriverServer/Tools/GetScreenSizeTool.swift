import CuaDriverCore
import Foundation
import MCP

public enum GetScreenSizeTool {
    public static let handler = ToolHandler(
        tool: Tool(
            name: "get_screen_size",
            description: """
                Return the logical size of the main display in points plus its backing scale
                factor. Agents click in points; Retina displays have scale_factor 2.0.
                Requires no TCC permissions.
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
            guard let size = ScreenInfo.mainScreenSize() else {
                return CallTool.Result(
                    content: [
                        .text(
                            text: "No main display detected.",
                            annotations: nil,
                            _meta: nil
                        )
                    ],
                    isError: true
                )
            }

            let summary = "✅ Main display: \(size.width)x\(size.height) points @ \(size.scaleFactor)x"
            return CallTool.Result(
                content: [.text(text: summary, annotations: nil, _meta: nil)]
            )
        }
    )
}
