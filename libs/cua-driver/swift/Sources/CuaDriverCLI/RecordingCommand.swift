import ArgumentParser
import CuaDriverServer
import Foundation
import MCP

/// `cua-driver recording <start|stop|status>` — convenience wrappers
/// around the `set_recording` / `get_recording_state` MCP tools so
/// trajectory-capture operations don't require hand-crafted JSON args.
///
/// Each child command forwards to a running daemon (no `--no-daemon`
/// escape: recording state is per-process, so running this against a
/// one-shot CLI would arm a recorder that dies immediately). The error
/// message on the unreachable path points the user at
/// `cua-driver serve`.
struct RecordingCommand: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "recording",
        abstract: "Control the trajectory recorder on a running cua-driver daemon.",
        discussion: """
            Wraps the `set_recording` and `get_recording_state` tools
            with human-friendly output. All subcommands require a
            running daemon (`cua-driver serve`) because recording
            state lives in-process and doesn't survive CLI-process
            lifetimes.

            Examples:
              cua-driver recording start ~/cua-trajectories/demo1
              cua-driver recording status
              cua-driver recording stop

            Turn folders appear under the start-time `<output-dir>`
            as `turn-00001/`, `turn-00002/`, …
            """,
        subcommands: [
            RecordingStartCommand.self,
            RecordingStopCommand.self,
            RecordingStatusCommand.self,
            RecordingRenderCommand.self,
        ]
    )
}

/// `cua-driver recording start <output-dir>` — enables the recorder
/// and points it at `<output-dir>`. Expands `~` and creates the
/// directory (+ intermediates) if missing.
struct RecordingStartCommand: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "start",
        abstract: "Enable trajectory recording to the given directory."
    )

    @Argument(
        help:
            "Directory to write turn folders into. Expands `~`; created if missing."
    )
    var outputDir: String

    @Flag(
        name: .customLong("video-experimental"),
        help: ArgumentHelp(
            "Also capture the main display to `<output-dir>/recording.mp4` via SCStream. H.264, 30fps, no audio, no cursor. Capture-only — no zoom / post-process. Experimental, off by default.",
            discussion: ""
        )
    )
    var videoExperimental: Bool = false

    @Option(name: .long, help: "Override the daemon Unix socket path.")
    var socket: String?

    func run() async throws {
        let socketPath = socket ?? DaemonPaths.defaultSocketPath()
        let expanded = (outputDir as NSString).expandingTildeInPath
        let absolute = URL(fileURLWithPath: expanded).standardizedFileURL.path

        // Best-effort pre-create so a path-literal typo (e.g. parent dir
        // missing) surfaces here before we round-trip through the daemon.
        // `set_recording` also creates the directory, so this is
        // belt-and-suspenders.
        do {
            try FileManager.default.createDirectory(
                atPath: absolute,
                withIntermediateDirectories: true,
                attributes: nil
            )
        } catch {
            printErr("Failed to create \(absolute): \(error.localizedDescription)")
            throw ExitCode(1)
        }

        guard DaemonClient.isDaemonListening(socketPath: socketPath) else {
            printErr(
                "cua-driver daemon is not running — start it with `cua-driver serve &`.")
            throw ExitCode(1)
        }

        var args: [String: Value] = [
            "enabled": .bool(true),
            "output_dir": .string(absolute),
        ]
        if videoExperimental {
            args["video_experimental"] = .bool(true)
        }
        let result = try await callDaemonTool(
            name: "set_recording", arguments: args, socketPath: socketPath
        )
        if result.isError == true {
            printErr(firstTextContent(result.content) ?? "set_recording failed")
            throw ExitCode(1)
        }

        // Follow up with a status query to get next_turn; set_recording
        // only returns enabled + output_dir.
        let state = try? await fetchRecordingState(socketPath: socketPath)
        var line = "Recording enabled -> \(absolute)"
        if let state, let nextTurn = state.nextTurn {
            line += "\nNext turn: \(nextTurn)"
        }
        if videoExperimental {
            line += "\nVideo: \(absolute)/recording.mp4 (experimental)"
        }
        print(line)
    }
}

/// `cua-driver recording stop` — disables the recorder. Prints the
/// last-recorded turn count and the directory it was writing into
/// when available (so the user knows where to look).
struct RecordingStopCommand: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "stop",
        abstract: "Disable trajectory recording."
    )

    @Option(name: .long, help: "Override the daemon Unix socket path.")
    var socket: String?

    func run() async throws {
        let socketPath = socket ?? DaemonPaths.defaultSocketPath()

        guard DaemonClient.isDaemonListening(socketPath: socketPath) else {
            printErr(
                "cua-driver daemon is not running — start it with `cua-driver serve &`.")
            throw ExitCode(1)
        }

        // Capture pre-stop state so the summary can include the turn
        // count and output directory that the recorder was using. After
        // `set_recording({enabled:false})` the recorder's state is
        // cleared, so the grab has to happen before the toggle.
        let priorState = try? await fetchRecordingState(socketPath: socketPath)

        let args: [String: Value] = ["enabled": .bool(false)]
        let result = try await callDaemonTool(
            name: "set_recording", arguments: args, socketPath: socketPath
        )
        if result.isError == true {
            printErr(firstTextContent(result.content) ?? "set_recording failed")
            throw ExitCode(1)
        }

        if let priorState,
            priorState.enabled,
            let dir = priorState.outputDir,
            let nextTurn = priorState.nextTurn
        {
            let captured = max(0, nextTurn - 1)
            print("Recording disabled (\(captured) turn\(captured == 1 ? "" : "s") captured in \(dir))")
        } else {
            print("Recording disabled.")
        }
    }
}

/// `cua-driver recording status` — prints whether recording is on.
/// Exits 0 either way — "disabled" is a valid state, not an error.
struct RecordingStatusCommand: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "status",
        abstract: "Report whether recording is currently enabled."
    )

    @Option(name: .long, help: "Override the daemon Unix socket path.")
    var socket: String?

    func run() async throws {
        let socketPath = socket ?? DaemonPaths.defaultSocketPath()

        guard DaemonClient.isDaemonListening(socketPath: socketPath) else {
            printErr(
                "cua-driver daemon is not running — start it with `cua-driver serve &`.")
            throw ExitCode(1)
        }

        let state = try await fetchRecordingState(socketPath: socketPath)
        if state.enabled, let dir = state.outputDir, let nextTurn = state.nextTurn {
            print("Recording: enabled")
            print("Output dir: \(dir)")
            print("Next turn: \(nextTurn)")
        } else {
            print("Recording: disabled")
        }
    }
}

// MARK: - Shared helpers

/// Decoded shape of `get_recording_state`'s structuredContent. Optional
/// fields map to the "disabled" response variant (no output_dir /
/// next_turn when recording is off).
private struct RecordingStateSnapshot {
    let enabled: Bool
    let outputDir: String?
    let nextTurn: Int?
}

/// Fetch the structuredContent from `get_recording_state` through the
/// daemon and project it into our local snapshot type. Throws
/// `ExitCode(1)` on protocol / transport failures so child commands
/// propagate a clean exit.
private func fetchRecordingState(socketPath: String) async throws
    -> RecordingStateSnapshot
{
    let result = try await callDaemonTool(
        name: "get_recording_state", arguments: nil, socketPath: socketPath
    )
    if result.isError == true {
        throw ExitCode(1)
    }
    let structured = result.structuredContent?.objectValue ?? [:]
    let enabled = structured["enabled"]?.boolValue ?? false
    let outputDir = structured["output_dir"]?.stringValue
    let nextTurn = structured["next_turn"]?.intValue
    return RecordingStateSnapshot(
        enabled: enabled, outputDir: outputDir, nextTurn: nextTurn
    )
}

/// Send a tool `call` to the daemon and unwrap it into a
/// `CallTool.Result`. Protocol-level failures throw; tool-level isError
/// responses come back in the result so the caller can format them.
private func callDaemonTool(
    name: String, arguments: [String: Value]?, socketPath: String
) async throws -> CallTool.Result {
    let request = DaemonRequest(method: "call", name: name, args: arguments)
    switch DaemonClient.sendRequest(request, socketPath: socketPath) {
    case .ok(let response):
        if !response.ok {
            printErr(response.error ?? "daemon reported failure")
            throw ExitCode(response.exitCode ?? 1)
        }
        guard case .call(let result) = response.result else {
            printErr("daemon returned unexpected result kind for call")
            throw ExitCode(1)
        }
        return result
    case .noDaemon:
        printErr(
            "cua-driver daemon disappeared — start it with `cua-driver serve &`.")
        throw ExitCode(1)
    case .error(let message):
        printErr("daemon error: \(message)")
        throw ExitCode(1)
    }
}

private func firstTextContent(_ content: [Tool.Content]) -> String? {
    for item in content {
        if case let .text(text, _, _) = item {
            return text
        }
    }
    return nil
}

private func printErr(_ text: String) {
    FileHandle.standardError.write(Data((text + "\n").utf8))
}
