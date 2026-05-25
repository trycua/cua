import ArgumentParser
import CuaDriverCore
import CuaDriverServer
import Foundation

struct DumpDocsCommand: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "dump-docs",
        abstract: "Output CLI and MCP tool documentation as JSON for tooling and integrations",
        discussion: """
            Extracts all command and MCP tool metadata including arguments, options, flags,
            and their help text, default values, and types. Useful for generating
            documentation or building integrations.

            Examples:
              cua-driver dump-docs                    # Output all docs
              cua-driver dump-docs --type cli         # Output CLI docs only
              cua-driver dump-docs --type mcp         # Output MCP tool docs only
              cua-driver dump-docs --pretty           # Pretty-print output
            """
    )

    @Option(help: "Documentation type to output: cli, mcp, or all")
    var type: DumpDocType = .all

    @Flag(help: "Pretty-print the JSON output with indentation")
    var pretty: Bool = false

    func run() throws {
        let encoder = JSONEncoder()
        encoder.keyEncodingStrategy = .convertToSnakeCase

        if pretty {
            encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        }

        let jsonData: Data

        switch type {
        case .cli:
            jsonData = try encoder.encode(CLIDocExtractor.extractAll())
        case .mcp:
            jsonData = try encoder.encode(MCPDocExtractor.extractAll())
        case .all:
            let combined = CombinedDocs(
                cli: CLIDocExtractor.extractAll(),
                mcp: MCPDocExtractor.extractAll()
            )
            jsonData = try encoder.encode(combined)
        }

        guard let jsonString = String(data: jsonData, encoding: .utf8) else {
            throw DumpDocsError.encodingFailed
        }

        print(jsonString)
    }
}

// MARK: - Documentation Type

enum DumpDocType: String, ExpressibleByArgument, CaseIterable {
    case cli
    case mcp
    case all
}

// MARK: - Combined Documentation

struct CombinedDocs: Codable {
    let cli: CLIDocumentation
    let mcp: MCPDocumentation
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
