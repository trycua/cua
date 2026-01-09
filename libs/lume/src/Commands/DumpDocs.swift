import ArgumentParser
import Foundation

/// Command to output CLI and API documentation as JSON for tooling and integrations
struct DumpDocs: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "dump-docs",
        abstract: "Output CLI and API documentation as JSON for tooling and integrations",
        discussion: """
            Extracts all command and API metadata including arguments, options, flags,
            endpoints, and their help text, default values, and types. Useful for
            generating documentation or building integrations.

            Examples:
              lume dump-docs                    # Output CLI docs
              lume dump-docs --type api         # Output HTTP API docs
              lume dump-docs --type all         # Output both CLI and API docs
              lume dump-docs --pretty           # Pretty-print output
            """
    )

    @Option(help: "Documentation type to output: cli, api, or all")
    var type: DocType = .cli

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
            let documentation = CommandDocExtractor.extractAll()
            jsonData = try encoder.encode(documentation)

        case .api:
            let documentation = APIDocExtractor.extractAll()
            jsonData = try encoder.encode(documentation)

        case .all:
            let combined = CombinedDocumentation(
                cli: CommandDocExtractor.extractAll(),
                api: APIDocExtractor.extractAll()
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

enum DocType: String, ExpressibleByArgument, CaseIterable {
    case cli
    case api
    case all
}

// MARK: - Combined Documentation

struct CombinedDocumentation: Codable {
    let cli: CLIDocumentation
    let api: HTTPAPIDocumentation
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
