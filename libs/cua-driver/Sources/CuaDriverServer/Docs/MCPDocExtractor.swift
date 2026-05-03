import CuaDriverCore
import Foundation
import MCP

// MARK: - MCP Documentation Types

/// Root documentation structure for MCP tools
public struct MCPDocumentation: Codable, Sendable {
    public let version: String
    public let tools: [MCPToolDoc]
}

/// Documentation for a single MCP tool
public struct MCPToolDoc: Codable, Sendable {
    public let name: String
    public let description: String
    public let inputSchema: Value
}

// MARK: - MCP Documentation Extractor

/// Serializes all MCP tools from `ToolRegistry.default` into a JSON-friendly structure.
public enum MCPDocExtractor {
    public static func extractAll() -> MCPDocumentation {
        let tools = ToolRegistry.default.allTools.map { tool in
            MCPToolDoc(
                name: tool.name,
                description: tool.description ?? "",
                inputSchema: tool.inputSchema
            )
        }
        return MCPDocumentation(
            version: CuaDriverCore.version,
            tools: tools
        )
    }
}
