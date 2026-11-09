import ArgumentParser
import Foundation
import Testing

@testable import lume

@Test("CLI reference covers every registered command")
func commandDocumentationCoversRegistry() {
    let coverage = CommandDocExtractor.documentationCoverage

    #expect(coverage.missing.isEmpty)
    #expect(coverage.extra.isEmpty)
    #expect(CommandDocExtractor.extractAll().commands.contains { $0.name == "sip" })
}

// MARK: - The hand-maintained tree matches ArgumentParser

/// The subset of ArgumentParser's `--experimental-dump-help` (ToolInfoV0)
/// the comparison needs.
private struct ToolInfo: Decodable {
    struct Name: Decodable {
        let kind: String
        let name: String
    }

    struct Argument: Decodable {
        let kind: String
        let names: [Name]?
        let valueName: String?
        let shouldDisplay: Bool
    }

    struct Command: Decodable {
        let commandName: String
        let shouldDisplay: Bool?
        let arguments: [Argument]?
        let subcommands: [Command]?
    }

    let command: Command
}

private struct Shape: Equatable, CustomStringConvertible {
    var arguments: [String] = []
    var options: [String] = []
    var flags: [String] = []

    var description: String {
        "arguments \(arguments), options \(options), flags \(flags)"
    }
}

private func toolInfo() throws -> ToolInfo {
    do {
        _ = try Lume.parseAsRoot(["--experimental-dump-help"])
    } catch {
        let json = Lume.fullMessage(for: error)
        return try JSONDecoder().decode(ToolInfo.self, from: Data(json.utf8))
    }
    throw ValidationError("--experimental-dump-help did not produce ToolInfo")
}

private func name(_ names: [ToolInfo.Name]?, _ kind: String) -> String? {
    names?.first { $0.kind == kind }?.name
}

/// Visible commands by path, from ArgumentParser (help and version excluded).
private func parserShapes(_ command: ToolInfo.Command, _ path: [String]) -> [String: Shape] {
    var out: [String: Shape] = [:]
    var shape = Shape()
    for argument in command.arguments ?? [] where argument.shouldDisplay {
        let long = name(argument.names, "long") ?? argument.valueName ?? ""
        if ["help", "version", "experimental-dump-help"].contains(long) { continue }
        let short = name(argument.names, "short").map { " -\($0)" } ?? ""
        switch argument.kind {
        case "positional": shape.arguments.append(argument.valueName ?? "")
        case "option": shape.options.append(long + short)
        default: shape.flags.append(long + short)
        }
    }
    if !path.isEmpty { out[path.joined(separator: " ")] = shape }
    for sub in command.subcommands ?? [] where sub.commandName != "help" && sub.shouldDisplay != false {
        out.merge(parserShapes(sub, path + [sub.commandName])) { a, _ in a }
    }
    return out
}

private func documentedShapes(_ commands: [CommandDoc], _ path: [String]) -> [String: Shape] {
    var out: [String: Shape] = [:]
    for command in commands {
        let here = path + [command.name]
        out[here.joined(separator: " ")] = Shape(
            arguments: command.arguments.map(\.name),
            options: command.options.map { $0.name + ($0.shortName.map { " -\($0)" } ?? "") },
            flags: command.flags.map { $0.name + ($0.shortName.map { " -\($0)" } ?? "") }
        )
        out.merge(documentedShapes(command.subcommands, here)) { a, _ in a }
    }
    return out
}

@Test("Documented arguments, options and flags match the ArgumentParser definitions")
func commandDocumentationMatchesParser() throws {
    let parsed = parserShapes(try toolInfo().command, [])
    let documented = documentedShapes(CommandDocExtractor.allCommandDocs, [])
    #expect(Swift.Set(parsed.keys) == Swift.Set(documented.keys))
    for (path, shape) in parsed {
        guard let doc = documented[path] else { continue }
        #expect(doc.arguments == shape.arguments, "lume \(path): arguments")
        #expect(Swift.Set(doc.options) == Swift.Set(shape.options), "lume \(path): options")
        #expect(Swift.Set(doc.flags) == Swift.Set(shape.flags), "lume \(path): flags")
    }
}

// MARK: - Examples parse

/// Splits a shell command line into words (single and double quotes).
private func shellWords(_ line: String) -> [String] {
    var words: [String] = []
    var current = ""
    var quote: Character?
    var inWord = false
    for ch in line {
        if let q = quote {
            if ch == q { quote = nil } else { current.append(ch) }
        } else if ch == "\"" || ch == "'" {
            quote = ch
            inWord = true
        } else if ch == " " {
            if inWord { words.append(current) }
            current = ""
            inWord = false
        } else {
            current.append(ch)
            inWord = true
        }
    }
    if inWord { words.append(current) }
    return words
}

private func allExamples(_ commands: [CommandDoc], _ path: [String]) -> [(String, [String])] {
    commands.flatMap { command -> [(String, [String])] in
        let here = path + [command.name]
        let mine = command.examples.map { (here.joined(separator: " "), shellWords($0.command)) }
        return mine + allExamples(command.subcommands, here)
    }
}

@Test("Every documented example parses and runs the command it documents")
func commandExamplesParse() throws {
    let examples = allExamples(CommandDocExtractor.allCommandDocs, [])
    #expect(!examples.isEmpty)
    for (path, words) in examples {
        #expect(words.first == "lume", "example for \(path) must start with lume: \(words)")
        let args = Array(words.dropFirst())
        let expected = path.split(separator: " ").map(String.init)
        #expect(Array(args.prefix(expected.count)) == expected, "example \(words) is not `lume \(path)`")
        do {
            _ = try Lume.parseAsRoot(args)
        } catch {
            Issue.record("`\(words.joined(separator: " "))` does not parse: \(Lume.message(for: error))")
        }
    }
}

@Test("Every leaf command has an example")
func everyLeafCommandHasAnExample() {
    func leaves(_ commands: [CommandDoc], _ path: [String]) -> [String] {
        commands.flatMap { command -> [String] in
            let here = path + [command.name]
            if command.subcommands.isEmpty {
                return command.examples.isEmpty ? [here.joined(separator: " ")] : []
            }
            return leaves(command.subcommands, here)
        }
    }
    #expect(leaves(CommandDocExtractor.allCommandDocs, []) == [])
}

@Test("MCP dump lists every tool with its schema and annotations")
func mcpDocumentationListsTools() throws {
    let docs = MCPDocumentation.extract()
    let names = docs.tools.map(\.name)
    #expect(names.contains("lume_create_vm"))
    #expect(Swift.Set(names).count == names.count)
    let delete = try #require(docs.tools.first { $0.name == "lume_delete_vm" })
    #expect(delete.annotations.destructive)
    let list = try #require(docs.tools.first { $0.name == "lume_list_vms" })
    #expect(list.annotations.readOnly)
    let json = try JSONEncoder().encode(docs)
    let object = try JSONSerialization.jsonObject(with: json) as? [String: Any]
    let first = (object?["tools"] as? [[String: Any]])?.first
    #expect(first?["input_schema"] != nil)
}
