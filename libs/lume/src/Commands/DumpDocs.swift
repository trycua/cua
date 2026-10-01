import ArgumentParser
import Foundation
import MCP

/// Command to output CLI and API documentation as JSON for tooling and integrations
struct DumpDocs: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "dump-docs",
        abstract: "Output CLI and API documentation as JSON for tooling and integrations",
        discussion: """
            Extracts all command, API and MCP tool metadata including arguments,
            options, flags, endpoints, tool schemas, and their help text, default
            values, and types. Useful for generating documentation or building
            integrations. Keys are always sorted, so the output is stable.

            Examples:
              lume dump-docs                    # Output CLI docs
              lume dump-docs --type api         # Output HTTP API docs
              lume dump-docs --type mcp         # Output the MCP server's tools
              lume dump-docs --type all         # Output CLI, API and MCP docs
              lume dump-docs --pretty           # Pretty-print output
            """
    )

    @Option(help: "Documentation type to output: cli, api, mcp, or all")
    var type: DocType = .cli

    @Flag(help: "Pretty-print the JSON output with indentation")
    var pretty: Bool = false

    func run() throws {
        // Snake-case keys for the Swift doc models; MCP schemas are emitted
        // verbatim (their keys are JSON Schema's own). Keys are always sorted
        // so every output is deterministic.
        let formatting: JSONEncoder.OutputFormatting = pretty ? [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes] : [.sortedKeys, .withoutEscapingSlashes]
        let snake = JSONEncoder()
        snake.keyEncodingStrategy = .convertToSnakeCase
        snake.outputFormatting = formatting
        let plain = JSONEncoder()
        plain.outputFormatting = formatting

        let jsonData: Data

        switch type {
        case .cli:
            jsonData = try snake.encode(CommandDocExtractor.extractAll())

        case .api:
            jsonData = try snake.encode(APIDocExtractor.extractAll())

        case .mcp:
            jsonData = try plain.encode(MCPDocumentation.extract())

        case .all:
            let parts: [String: Any] = [
                "cli": try JSONSerialization.jsonObject(with: snake.encode(CommandDocExtractor.extractAll())),
                "api": try JSONSerialization.jsonObject(with: snake.encode(APIDocExtractor.extractAll())),
                "mcp": try JSONSerialization.jsonObject(with: plain.encode(MCPDocumentation.extract())),
            ]
            var options: JSONSerialization.WritingOptions = [.sortedKeys, .withoutEscapingSlashes]
            if pretty { options.insert(.prettyPrinted) }
            jsonData = try JSONSerialization.data(withJSONObject: parts, options: options)
        }

        guard let jsonString = String(data: jsonData, encoding: .utf8) else {
            throw DumpDocsError.encodingFailed
        }

        print(jsonString)
    }
}

// MARK: - MCP Documentation

/// `lume dump-docs --type mcp`: the MCP server's tool list, read from
/// `LumeMCPServer.toolDefinitions` without starting a server.
struct MCPDocumentation: Encodable {
    let version: String
    let tools: [MCPToolDoc]

    static func extract() -> MCPDocumentation {
        MCPDocumentation(
            version: Lume.Version.current,
            tools: LumeMCPServer.toolDefinitions.map { tool in
                MCPToolDoc(
                    name: tool.name,
                    description: tool.description ?? "",
                    inputSchema: tool.inputSchema,
                    annotations: MCPToolAnnotationsDoc(
                        readOnly: tool.annotations.readOnlyHint ?? false,
                        destructive: tool.annotations.destructiveHint ?? false,
                        idempotent: tool.annotations.idempotentHint ?? false
                    )
                )
            }
        )
    }
}

struct MCPToolDoc: Encodable {
    let name: String
    let description: String
    let inputSchema: Value
    let annotations: MCPToolAnnotationsDoc

    enum CodingKeys: String, CodingKey {
        case name, description, annotations
        case inputSchema = "input_schema"
    }
}

struct MCPToolAnnotationsDoc: Encodable {
    let readOnly: Bool
    let destructive: Bool
    let idempotent: Bool

    enum CodingKeys: String, CodingKey {
        case destructive, idempotent
        case readOnly = "read_only"
    }
}

// MARK: - Documentation Type

enum DocType: String, ExpressibleByArgument, CaseIterable {
    case cli
    case api
    case mcp
    case all
}

// MARK: - Errors

enum DumpDocsError: Error, CustomStringConvertible {
    case encodingFailed

    var description: String {
        switch self {
        case .encodingFailed:
            return "Failed to encode documentation to JSON"
        }
    }
}
