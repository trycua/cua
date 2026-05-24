import CuaDriverCore
import Foundation
import MCP

/// Set the visual style of the agent-cursor overlay. All fields are
/// optional — omit any field to keep its current value.
///
/// Changes take effect on the next rendered frame. Style choices are
/// persisted to config so the next daemon restart restores them.
public enum SetAgentCursorStyleTool {
    public static let handler = ToolHandler(
        tool: Tool(
            name: "set_agent_cursor_style",
            description: """
                Customize the agent-cursor's visual appearance. All fields
                optional; omitted fields keep their current value.

                - gradient_colors: Array of CSS hex strings (#RRGGBB or
                  #RGB) defining the arrow fill gradient from tip to tail.
                  E.g. ["#FF6B6B", "#FF8E53"] for a red-orange arrow.
                - bloom_color: CSS hex string for the glow halo and the
                  focus-rect highlight drawn around clicked elements.
                - image_path: Absolute or ~-rooted path to a PNG, JPEG,
                  PDF, or SVG file. When set, replaces the default arrow
                  with this image (drawn at shapeSize × shapeSize points,
                  rotated to match the motion heading). Set to "" (empty
                  string) to revert to the procedural arrow.

                Example — brand-colored arrow:
                  {"gradient_colors": ["#A855F7", "#6366F1"], "bloom_color": "#A855F7"}

                Example — custom PNG cursor:
                  {"image_path": "~/cursors/my-cursor.png"}

                Example — revert to default:
                  {"gradient_colors": [], "bloom_color": "", "image_path": ""}
                """,
            inputSchema: [
                "type": "object",
                "properties": [
                    "gradient_colors": [
                        "type": "array",
                        "items": ["type": "string"],
                        "description": "CSS hex color strings for gradient stops (tip to tail). Empty array reverts to default.",
                    ],
                    "bloom_color": [
                        "type": "string",
                        "description": "CSS hex color for the bloom halo and focus rect. Empty string reverts to default.",
                    ],
                    "image_path": [
                        "type": "string",
                        "description": "Path to PNG/JPEG/PDF/SVG cursor image. Empty string reverts to arrow.",
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
            // Read current persisted style; we'll mutate only the supplied fields.
            var styleConfig = await ConfigStore.shared.load().agentCursor.style

            if let colors = arguments?["gradient_colors"]?.arrayValue {
                let hexes = colors.compactMap { $0.stringValue }
                styleConfig.gradientColors = hexes.isEmpty ? nil : hexes
            }

            if let bloomStr = arguments?["bloom_color"]?.stringValue {
                styleConfig.bloomColor = bloomStr.isEmpty ? nil : bloomStr
            }

            if let pathStr = arguments?["image_path"]?.stringValue {
                styleConfig.imagePath = pathStr.isEmpty ? nil : pathStr
            }

            // Apply live so the change is immediate.
            await MainActor.run {
                AgentCursor.shared.applyStyleConfig(styleConfig)
            }

            // Persist.
            do {
                try await ConfigStore.shared.mutate { config in
                    config.agentCursor.style = styleConfig
                }
            } catch {
                return CallTool.Result(
                    content: [.text(
                        text: "Style applied live but persisting to config failed: \(error.localizedDescription)",
                        annotations: nil, _meta: nil
                    )],
                    isError: true
                )
            }

            var parts: [String] = []
            if let gc = styleConfig.gradientColors { parts.append("gradient_colors=[\(gc.joined(separator: ","))]") }
            if let bc = styleConfig.bloomColor { parts.append("bloom_color=\(bc)") }
            if let ip = styleConfig.imagePath { parts.append("image_path=\(ip)") }
            let summary = parts.isEmpty ? "reverted to default" : parts.joined(separator: " ")
            return CallTool.Result(
                content: [.text(text: "✅ cursor style: \(summary)", annotations: nil, _meta: nil)]
            )
        }
    )
}
