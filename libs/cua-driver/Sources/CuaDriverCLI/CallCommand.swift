import ArgumentParser
import CuaDriverServer
import Foundation
import MCP

// Exit codes — sysexits.h equivalents so shell users can branch on them.
private enum Exit {
    static let toolError: Int32 = 1       // tool ran, returned isError: true
    static let usage: Int32 = 64          // EX_USAGE — unknown tool / missing arg
    static let dataError: Int32 = 65      // EX_DATAERR — JSON parse failure
    static let software: Int32 = 70       // EX_SOFTWARE — tool threw / internal error
}

/// Shared flag block for JSON output formatting. Keeps `call`, `list-tools`,
/// and `describe` consistent: default pretty, `--compact` opts out.
struct JSONOutputOptions: ParsableArguments {
    @Flag(name: .long, help: "Emit minified JSON instead of pretty-printed.")
    var compact: Bool = false

    var encoderOutputFormatting: JSONEncoder.OutputFormatting {
        if compact {
            return [.sortedKeys, .withoutEscapingSlashes]
        }
        return [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
    }
}

/// Shared flag block controlling whether the CLI forwards to a running
/// `cua-driver serve` daemon when one is reachable on the socket. Every
/// stateful CLI op (call/list-tools/describe) picks up these flags so
/// users can force in-process behavior for debugging.
struct DaemonForwardingOptions: ParsableArguments {
    @Flag(name: .long, help: "Skip the cua-driver daemon even if one is running.")
    var noDaemon: Bool = false

    @Option(name: .long, help: "Override the daemon Unix socket path.")
    var socket: String?

    var resolvedSocketPath: String {
        socket ?? DaemonPaths.defaultSocketPath()
    }

    /// Returns a reachable DaemonClient probe decision. The `--no-daemon`
    /// flag short-circuits; otherwise we try to connect. Refusing to forward
    /// on missing socket files keeps tests that don't start a daemon from
    /// accidentally hitting a stale one.
    var shouldForwardToDaemon: Bool {
        if noDaemon { return false }
        return DaemonClient.isDaemonListening(socketPath: resolvedSocketPath)
    }
}

struct CallCommand: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "call",
        abstract: "Invoke an MCP tool directly from the shell.",
        discussion: """
            Runs the same ToolHandler the MCP server uses. JSON-ARGS is a JSON
            object mapping to the tool's inputSchema. When JSON-ARGS is omitted
            and stdin is a pipe, JSON is read from stdin. When neither is
            provided, the tool is called with no arguments.

            Examples:
              cua-driver call list_apps
              cua-driver call launch_app '{"bundle_id":"com.apple.finder"}'
              echo '{"pid":844,"window_id":1234}' | cua-driver call get_window_state
            """
    )

    @Argument(help: "Name of the tool to invoke (see `cua-driver list-tools`).")
    var toolName: String

    @Argument(help: "JSON object string for the tool's inputSchema. If omitted, reads from stdin when stdin is a pipe.")
    var jsonArgs: String?

    @Flag(name: .long, help: "Print the raw CallTool.Result JSON (content + structuredContent + isError) instead of unwrapping structuredContent.")
    var raw: Bool = false

    @Option(
        name: .long,
        help: """
            Write the first image content block from the response to this
            file path. `vision` and `screenshot` capture modes return a
            PNG as a native MCP image block with no accompanying
            structuredContent, so the CLI's text formatter would otherwise
            drop the bytes silently. With `--screenshot-out-file /tmp/shot.jpg`
            the raw image lands on disk and downstream tooling (PIL, sips,
            ffprobe) can read it directly. Silently warns (no error exit)
            when the response carries no image.
            """
    )
    var screenshotOutFile: String?

    @OptionGroup var output: JSONOutputOptions
    @OptionGroup var daemon: DaemonForwardingOptions

    func run() async throws {
        let arguments: [String: Value]?
        do {
            arguments = try decodeArguments(positional: jsonArgs)
        } catch let error as ArgumentDecodeError {
            printToStderr("Failed to parse JSON arguments: \(error.message)")
            throw ExitCode(Exit.dataError)
        }

        // Auto-forward to the daemon when one is up. The daemon dispatches
        // against the same ToolRegistry but keeps AppStateRegistry state
        // alive between our short-lived CLI invocations — the reason
        // multi-step element_index flows work at all from the shell.
        if daemon.shouldForwardToDaemon {
            try await forwardCallToDaemon(
                toolName: toolName,
                arguments: arguments,
                socketPath: daemon.resolvedSocketPath,
                raw: raw,
                imageOut: screenshotOutFile,
                output: output
            )
            return
        }

        try await runInProcess(arguments: arguments)
    }

    private func runInProcess(arguments: [String: Value]?) async throws {
        let registry = ToolRegistry.default
        var arguments = arguments

        guard let handler = registry.handlers[toolName] else {
            printUnknownTool(toolName, registry: registry)
            throw ExitCode(Exit.usage)
        }

        // Validate required fields up front — lets us emit a consistent
        // JSON-shaped error (exit 65) for "schema says pid is required and
        // you didn't supply it" rather than letting that bubble up as a
        // tool isError exit 1.
        if let missing = missingRequiredFields(
            schema: handler.tool.inputSchema, arguments: arguments
        ) {
            printToStderr(
                "Missing required field(s) for \(toolName): \(missing.joined(separator: ", "))"
            )
            throw ExitCode(Exit.dataError)
        }

        // `check_permissions` in-process is ONLY correct when the process
        // is running from CuaDriver.app (the daemon). Any one-shot CLI
        // spawned by an IDE terminal (Conductor, VS Code, Cursor, Claude
        // Code) inherits the IDE's TCC responsibility chain, so
        // AXIsProcessTrusted() / SCShareableContent.current read against
        // the IDE's bundle — not com.trycua.driver — and report
        // "NOT granted" even when the user has granted both permissions
        // to CuaDriver.app. If the daemon were up we'd already have
        // forwarded in run(); reaching this branch means no daemon is
        // listening. Warn the user that the fallback answer is unreliable,
        // and force `prompt: false` — the tool's default would otherwise
        // raise a TCC dialog attributed to the calling shell/IDE bundle,
        // not CuaDriver.app, so the user would grant the wrong identity.
        if toolName == "check_permissions" {
            printToStderr(
                """
                ⚠️ Not running inside the cua-driver daemon process. Results may be inaccurate — TCC checks the calling process, not CuaDriver.app, so permissions granted to CuaDriver.app may read as "NOT granted" here.
                For authoritative results, start the daemon first: `open -n -g -a CuaDriver --args serve`, then re-run this check.
                """
            )
            var coerced = arguments ?? [:]
            coerced["prompt"] = .bool(false)
            arguments = coerced
        }

        let result: CallTool.Result
        do {
            // Route through `registry.call(...)` so the recording hook
            // (and any future cross-cutting wrapper) fires consistently
            // with the MCP and daemon paths. The in-process one-shot
            // is the odd path out — resetting recording every run —
            // but that's fine: `set_recording` itself is what arms
            // the hook, and it's only meaningful when the daemon
            // persists state anyway.
            result = try await registry.call(toolName, arguments: arguments)
        } catch {
            printToStderr("Tool \(toolName) threw: \(error)")
            throw ExitCode(Exit.software)
        }

        if let path = screenshotOutFile {
            writeFirstImageContent(result.content, to: path)
        }

        if raw {
            try emitRawResult(result)
        } else {
            try emitUnwrappedResult(result)
        }

        if result.isError == true {
            // emitUnwrappedResult already printed the tool's error text to
            // stderr in the default path; raw mode intentionally still
            // surfaces exit 1 so callers can detect failure.
            throw ExitCode(Exit.toolError)
        }
    }

    /// Turn positional / piped JSON into `[String: Value]`. Returns `nil` when
    /// there are genuinely no arguments (no positional, no piped stdin).
    private func decodeArguments(positional: String?) throws -> [String: Value]? {
        let source: String?
        if let positional, !positional.isEmpty {
            source = positional
        } else if !isStdinTTY() {
            let piped = readStdinToEndAsString()
            source = piped.isEmpty ? nil : piped
        } else {
            source = nil
        }

        guard let jsonString = source else { return nil }

        let data = Data(jsonString.utf8)
        do {
            let decoded = try JSONDecoder().decode([String: Value].self, from: data)
            return decoded
        } catch {
            throw ArgumentDecodeError(
                message: "\(error.localizedDescription) (input: \(jsonString.prefix(200)))"
            )
        }
    }

    private func emitRawResult(_ result: CallTool.Result) throws {
        let encoder = JSONEncoder()
        encoder.outputFormatting = output.encoderOutputFormatting
        let data = try encoder.encode(result)
        printDataLine(data)
    }

    /// Unwrap the tool result:
    /// - success: print `structuredContent` as JSON if present, else the
    ///   concatenated text content.
    /// - error: print `content[0].text` to stderr.
    private func emitUnwrappedResult(_ result: CallTool.Result) throws {
        if result.isError == true {
            let text = firstTextContent(result.content)
                ?? "Tool reported an error with no text content."
            printToStderr(text)
            return
        }

        if let structured = result.structuredContent {
            let encoder = JSONEncoder()
            encoder.outputFormatting = output.encoderOutputFormatting
            let encoded = try encoder.encode(structured)
            printDataLine(encoded)
            return
        }

        // No structuredContent — fall back to text blobs. Tools that don't
        // carry structured output (rare) still give something useful.
        let text = allTextContent(result.content)
        if !text.isEmpty {
            FileHandle.standardOutput.write(Data((text + "\n").utf8))
        }
    }
}

struct ListToolsCommand: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "list-tools",
        abstract: "List every tool exposed by cua-driver with its one-line description."
    )

    @OptionGroup var daemon: DaemonForwardingOptions

    func run() async throws {
        let tools: [Tool]
        if daemon.shouldForwardToDaemon {
            tools = try fetchToolsFromDaemon(socketPath: daemon.resolvedSocketPath)
        } else {
            tools = ToolRegistry.default.allTools
        }

        var buffer = ""
        for tool in tools.sorted(by: { $0.name < $1.name }) {
            let summary = firstSentence(tool.description ?? "")
            let line = summary.isEmpty ? tool.name : "\(tool.name): \(summary)"
            buffer += line + "\n"
        }
        FileHandle.standardOutput.write(Data(buffer.utf8))
    }
}

struct DescribeCommand: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "describe",
        abstract: "Print a tool's full description and JSON input schema."
    )

    @Argument(help: "Name of the tool to describe.")
    var toolName: String

    @OptionGroup var output: JSONOutputOptions
    @OptionGroup var daemon: DaemonForwardingOptions

    func run() async throws {
        let tool: Tool
        if daemon.shouldForwardToDaemon {
            tool = try fetchToolDescriptionFromDaemon(
                name: toolName,
                socketPath: daemon.resolvedSocketPath
            )
        } else {
            let registry = ToolRegistry.default
            guard let handler = registry.handlers[toolName] else {
                printUnknownTool(toolName, registry: registry)
                throw ExitCode(Exit.usage)
            }
            tool = handler.tool
        }

        var lines = "name: \(tool.name)\n"
        if let description = tool.description, !description.isEmpty {
            lines += "\ndescription:\n"
            lines += description
            if !description.hasSuffix("\n") { lines += "\n" }
        }
        lines += "\ninput_schema:\n"
        FileHandle.standardOutput.write(Data(lines.utf8))

        let encoder = JSONEncoder()
        encoder.outputFormatting = output.encoderOutputFormatting
        let data = try encoder.encode(tool.inputSchema)
        printDataLine(data)
    }
}

// MARK: - helpers

private struct ArgumentDecodeError: Error {
    let message: String
}

// MARK: - daemon forwarding

/// Forward a `call` request to the running daemon, print the result
/// identically to the in-process path, and propagate the right exit code.
func forwardCallToDaemon(
    toolName: String,
    arguments: [String: Value]?,
    socketPath: String,
    raw: Bool,
    screenshotOutFile: String?,
    output: JSONOutputOptions
) async throws {
    let request = DaemonRequest(method: "call", name: toolName, args: arguments)
    switch DaemonClient.sendRequest(request, socketPath: socketPath) {
    case .noDaemon:
        // Race: daemon disappeared between our probe and the write. Fall
        // back to in-process by throwing a distinct error the caller
        // could catch — simpler here: just report.
        FileHandle.standardError.write(
            Data("daemon went away while sending request\n".utf8)
        )
        throw ExitCode(Exit.software)
    case .error(let message):
        FileHandle.standardError.write(Data("\(message)\n".utf8))
        throw ExitCode(Exit.software)
    case .ok(let response):
        if !response.ok {
            let text = response.error ?? "daemon reported failure"
            FileHandle.standardError.write(Data((text + "\n").utf8))
            throw ExitCode(response.exitCode ?? Exit.software)
        }
        guard case .call(let result) = response.result else {
            FileHandle.standardError.write(
                Data("daemon returned unexpected result kind for call\n".utf8)
            )
            throw ExitCode(Exit.software)
        }

        if let path = screenshotOutFile {
            writeFirstImageContent(result.content, to: path)
        }

        if raw {
            let encoder = JSONEncoder()
            encoder.outputFormatting = output.encoderOutputFormatting
            let data = try encoder.encode(result)
            FileHandle.standardOutput.write(data)
            FileHandle.standardOutput.write(Data("\n".utf8))
        } else {
            try emitUnwrappedResultForDaemon(result, output: output)
        }

        if result.isError == true {
            throw ExitCode(Exit.toolError)
        }
    }
}

/// Mirror of CallCommand.emitUnwrappedResult, callable from free functions.
/// Kept in sync with the in-process path's formatting choices (pretty by
/// default, compact under --compact, fall back to text when no
/// structuredContent is present).
private func emitUnwrappedResultForDaemon(
    _ result: CallTool.Result, output: JSONOutputOptions
) throws {
    if result.isError == true {
        var text: String? = nil
        for item in result.content {
            if case let .text(t, _, _) = item {
                text = t
                break
            }
        }
        FileHandle.standardError.write(
            Data(((text ?? "Tool reported an error with no text content.") + "\n").utf8)
        )
        return
    }

    if let structured = result.structuredContent {
        let encoder = JSONEncoder()
        encoder.outputFormatting = output.encoderOutputFormatting
        let encoded = try encoder.encode(structured)
        FileHandle.standardOutput.write(encoded)
        FileHandle.standardOutput.write(Data("\n".utf8))
        return
    }

    var parts: [String] = []
    for item in result.content {
        if case let .text(t, _, _) = item {
            parts.append(t)
        }
    }
    let text = parts.joined(separator: "\n")
    if !text.isEmpty {
        FileHandle.standardOutput.write(Data((text + "\n").utf8))
    }
}

func fetchToolsFromDaemon(socketPath: String) throws -> [Tool] {
    let request = DaemonRequest(method: "list")
    switch DaemonClient.sendRequest(request, socketPath: socketPath) {
    case .ok(let response):
        guard response.ok, case .list(let tools) = response.result else {
            throw DaemonCLIError.protocolMismatch
        }
        return tools
    case .noDaemon, .error:
        throw DaemonCLIError.unreachable
    }
}

func fetchToolDescriptionFromDaemon(name: String, socketPath: String) throws -> Tool {
    let request = DaemonRequest(method: "describe", name: name)
    switch DaemonClient.sendRequest(request, socketPath: socketPath) {
    case .ok(let response):
        if !response.ok {
            if response.exitCode == Exit.usage,
                let error = response.error, error.contains("Unknown tool") {
                FileHandle.standardError.write(Data((error + "\n").utf8))
                throw ExitCode(Exit.usage)
            }
            let text = response.error ?? "daemon describe failed"
            FileHandle.standardError.write(Data((text + "\n").utf8))
            throw ExitCode(response.exitCode ?? Exit.software)
        }
        guard case .describe(let tool) = response.result else {
            throw DaemonCLIError.protocolMismatch
        }
        return tool
    case .noDaemon, .error:
        throw DaemonCLIError.unreachable
    }
}

enum DaemonCLIError: Error {
    case unreachable
    case protocolMismatch
}


/// Write the first `.image(...)` content block from a tool result to
/// `path`, decoded from base64. Used by the `--screenshot-out-file` flag so
/// vision-mode screenshots land on disk without callers needing to
/// speak the raw daemon protocol or the MCP bridge.
///
/// Never throws — an empty/malformed image, a write failure, or a
/// response that carries no image at all each emit a stderr warning
/// and return. The exit code is NOT affected: callers have already
/// gotten the textual result on stdout, and `--screenshot-out-file` is
/// advisory ("if the tool returned a PNG, put it here"), not a hard
/// contract.
func writeFirstImageContent(_ content: [Tool.Content], to path: String) {
    for item in content {
        if case let .image(base64, mime, _, _) = item {
            guard let bytes = Data(base64Encoded: base64) else {
                printToStderr("--screenshot-out-file: base64 decode failed for \(mime) block")
                return
            }
            let url = URL(fileURLWithPath: (path as NSString).expandingTildeInPath)
            do {
                try bytes.write(to: url)
            } catch {
                printToStderr(
                    "--screenshot-out-file: failed to write \(url.path): \(error.localizedDescription)"
                )
            }
            return
        }
    }
    // Response carried no image content block. Not an error — some
    // tools legitimately return nothing (hidden/minimized windows,
    // `ax` capture mode, list operations). Warn so the user notices
    // the file they expected didn't get written.
    printToStderr("--screenshot-out-file: no image content in tool response; file not written")
}

private func printUnknownTool(_ name: String, registry: ToolRegistry) {
    printToStderr("Unknown tool: \(name)")
    printToStderr("Available tools:")
    for toolName in registry.allTools.map(\.name).sorted() {
        printToStderr("  \(toolName)")
    }
}

private func printToStderr(_ text: String) {
    FileHandle.standardError.write(Data((text + "\n").utf8))
}

private func printDataLine(_ data: Data) {
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data("\n".utf8))
}

private func isStdinTTY() -> Bool {
    isatty(fileno(stdin)) != 0
}

private func readStdinToEndAsString() -> String {
    let data = FileHandle.standardInput.readDataToEndOfFile()
    return String(data: data, encoding: .utf8) ?? ""
}

private func firstTextContent(_ content: [Tool.Content]) -> String? {
    for item in content {
        if case let .text(text, _, _) = item {
            return text
        }
    }
    return nil
}

private func allTextContent(_ content: [Tool.Content]) -> String {
    var parts: [String] = []
    for item in content {
        if case let .text(text, _, _) = item {
            parts.append(text)
        }
    }
    return parts.joined(separator: "\n")
}

/// Return the list of required-schema keys missing from `arguments`, or
/// nil when nothing is missing. Reads a standard JSON-Schema `required`
/// array from the tool's inputSchema.
private func missingRequiredFields(
    schema: Value, arguments: [String: Value]?
) -> [String]? {
    guard case let .object(fields) = schema,
        case let .array(requiredValues) = fields["required"] ?? .null
    else {
        return nil
    }
    let requiredKeys = requiredValues.compactMap { $0.stringValue }
    if requiredKeys.isEmpty { return nil }

    let args = arguments ?? [:]
    let missing = requiredKeys.filter { args[$0] == nil }
    return missing.isEmpty ? nil : missing
}

/// Best-effort "first sentence" pull from a tool description. Tool
/// descriptions in this project are multi-paragraph; we want the first
/// sentence of the first paragraph for `list-tools`.
private func firstSentence(_ text: String) -> String {
    let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
    if trimmed.isEmpty { return "" }

    // Collapse to the first paragraph so multi-paragraph descriptions
    // don't leak later context into the summary.
    let firstPara = trimmed.components(separatedBy: "\n\n").first
        ?? trimmed
    // Flatten wrapped lines in that paragraph — the inputs use
    // triple-quoted string literals with hard wraps.
    let flat = firstPara
        .replacingOccurrences(of: "\n", with: " ")
        .trimmingCharacters(in: .whitespaces)

    // Scan for the first sentence terminator followed by whitespace.
    var sentence = ""
    var prev: Character = " "
    for ch in flat {
        if (prev == "." || prev == "?" || prev == "!")
            && (ch == " " || ch == "\n" || ch == "\t") {
            break
        }
        sentence.append(ch)
        prev = ch
    }
    sentence = sentence.trimmingCharacters(in: .whitespacesAndNewlines)
    if sentence.hasSuffix(".") {
        sentence.removeLast()
    }
    return sentence
}
