import CuaDriverCore
import Foundation
import MCP

/// Re-emit a previously recorded trajectory's tool calls in order against
/// the live daemon / MCP server. Walks `<dir>/turn-NNNNN/` folders in
/// lexical order, reads each `action.json`, and invokes the named tool
/// with its recorded `arguments` via the same dispatch path the MCP
/// handler uses — so replayed actions flow through the recording hook
/// too. If recording is enabled during replay, the replay itself gets
/// recorded (regression-diffing is the intended use). `replay_trajectory`
/// is deliberately excluded from the action-tool set so it doesn't try
/// to record itself, which would infinite-loop.
///
/// The intended companion to `set_recording` / `cua-driver recording
/// start`. Use case: capture a session once, then replay against a
/// future build and diff the two trajectories.
public enum ReplayTrajectoryTool {
    public static let handler = ToolHandler(
        tool: Tool(
            name: "replay_trajectory",
            description: """
                Replay a recorded trajectory by re-invoking every turn's
                tool call in lexical order. `dir` must point at a
                directory previously written by `set_recording` (or the
                `cua-driver recording start` CLI). Each `turn-NNNNN/`
                is parsed for `action.json`, and the recorded tool is
                called with its recorded `arguments` via the same
                dispatch path an MCP / CLI call uses.

                Caveats:

                - Element-indexed actions (`click({pid, element_index})`
                  etc.) will fail because element indices are
                  per-snapshot and don't survive across sessions. Pixel
                  clicks (`click({pid, x, y})`) and all keyboard tools
                  replay cleanly. Failures are reported but don't stop
                  replay unless `stop_on_error` is true.
                - `get_window_state` and other read-only tools are NOT
                  currently recorded, so replays do not re-populate the
                  per-(pid, window_id) element cache. A recorded session
                  consisting solely of element-indexed clicks will
                  therefore replay as a stream of "No cached AX state"
                  errors — fine for regression diffs, useless for
                  re-driving an app.
                - If recording is ENABLED while replay runs, the replay
                  itself is recorded into the currently configured
                  output directory. That's deliberate: recording a
                  replay against a new build and diffing the two
                  trajectories is the regression-test workflow.
                """,
            inputSchema: [
                "type": "object",
                "required": ["dir"],
                "properties": [
                    "dir": [
                        "type": "string",
                        "description":
                            "Trajectory directory previously written by `set_recording`. Absolute or ~-rooted.",
                    ],
                    "delay_ms": [
                        "type": "integer",
                        "description":
                            "Milliseconds to sleep between turns, for human-observable pacing. Default 500.",
                        "minimum": 0,
                        "maximum": 10_000,
                    ],
                    "stop_on_error": [
                        "type": "boolean",
                        "description":
                            "Stop replay on the first tool-call error. Default true — set false to best-effort through the full trajectory.",
                    ],
                ],
                "additionalProperties": false,
            ],
            annotations: .init(
                readOnlyHint: false,
                destructiveHint: true,
                idempotentHint: false,
                openWorldHint: false
            )
        ),
        invoke: { arguments in
            guard let rawDir = arguments?["dir"]?.stringValue, !rawDir.isEmpty
            else {
                return errorResult("Missing required string field `dir`.")
            }
            let expanded = (rawDir as NSString).expandingTildeInPath
            let dirURL = URL(fileURLWithPath: expanded).standardizedFileURL

            var isDirectory: ObjCBool = false
            guard
                FileManager.default.fileExists(
                    atPath: dirURL.path, isDirectory: &isDirectory
                ),
                isDirectory.boolValue
            else {
                return errorResult(
                    "Trajectory directory does not exist: \(dirURL.path)")
            }

            let delayMs: Int = {
                if let v = arguments?["delay_ms"]?.intValue { return max(0, v) }
                return 500
            }()
            let stopOnError: Bool = arguments?["stop_on_error"]?.boolValue ?? true

            let turnDirs: [URL]
            do {
                turnDirs = try listTurnDirectories(in: dirURL)
            } catch {
                return errorResult(
                    "Failed to enumerate trajectory directory: \(error.localizedDescription)")
            }
            if turnDirs.isEmpty {
                return errorResult(
                    "No turn-NNNNN folders found under \(dirURL.path).")
            }

            var attempted = 0
            var succeeded = 0
            var failed = 0
            var firstFailure: ReplayFailure?

            for (index, turnDir) in turnDirs.enumerated() {
                let turnLabel = turnDir.lastPathComponent
                let actionURL = turnDir.appendingPathComponent("action.json")

                guard let parsed = parseActionJSON(at: actionURL) else {
                    // Malformed / missing action.json — log and continue
                    // (every valid turn has one; a missing file usually
                    // means the recorder crashed mid-turn).
                    FileHandle.standardError.write(
                        Data(
                            "replay_trajectory: skipped \(turnLabel) (unreadable action.json)\n"
                                .utf8)
                    )
                    continue
                }
                attempted += 1

                let result: CallTool.Result
                do {
                    // Dispatch lazily through the shared registry accessor.
                    // Can't reference `ToolRegistry.default` directly at
                    // handler-init time — it includes this very tool in
                    // its handler list, which Swift flags as a circular
                    // static reference. The accessor indirects through a
                    // function so the dependency edge only fires at call
                    // time (the compiler sees a function body it doesn't
                    // have to evaluate statically).
                    let registry = ToolRegistry.shared()
                    result = try await registry.call(
                        parsed.toolName, arguments: parsed.arguments
                    )
                } catch {
                    failed += 1
                    let message = "\(error)"
                    if firstFailure == nil {
                        firstFailure = ReplayFailure(
                            turn: turnLabel, tool: parsed.toolName, error: message
                        )
                    }
                    if stopOnError {
                        break
                    } else {
                        continue
                    }
                }

                if result.isError == true {
                    failed += 1
                    let text = firstTextContent(result.content)
                        ?? "tool reported isError with no text content"
                    if firstFailure == nil {
                        firstFailure = ReplayFailure(
                            turn: turnLabel, tool: parsed.toolName, error: text
                        )
                    }
                    if stopOnError { break }
                } else {
                    succeeded += 1
                }

                // Pacing between turns. Skip after the last one — no
                // reason to make the tool return slower than it has to.
                if delayMs > 0, index < turnDirs.count - 1 {
                    try? await Task.sleep(
                        nanoseconds: UInt64(delayMs) * 1_000_000
                    )
                }
            }

            let summary: String
            if let firstFailure {
                summary =
                    "replay \(dirURL.lastPathComponent): attempted=\(attempted) "
                    + "succeeded=\(succeeded) failed=\(failed) "
                    + "first_failure=\(firstFailure.turn):\(firstFailure.tool)"
            } else {
                summary =
                    "replay \(dirURL.lastPathComponent): attempted=\(attempted) "
                    + "succeeded=\(succeeded) failed=\(failed)"
            }

            var structured: [String: Value] = [
                "turns_attempted": .int(attempted),
                "turns_succeeded": .int(succeeded),
                "turns_failed": .int(failed),
            ]
            if let firstFailure {
                structured["first_failure"] = .object([
                    "turn": .string(firstFailure.turn),
                    "tool": .string(firstFailure.tool),
                    "error": .string(firstFailure.error),
                ])
            }

            return CallTool.Result(
                content: [.text(text: "✅ \(summary)", annotations: nil, _meta: nil)]
            )
        }
    )

    // MARK: - Helpers

    private struct ReplayFailure {
        let turn: String
        let tool: String
        let error: String
    }

    private struct ParsedAction {
        let toolName: String
        let arguments: [String: Value]?
    }

    /// Enumerate `turn-*` subdirectories in lexical order. Non-directory
    /// entries and anything not matching the prefix are skipped.
    private static func listTurnDirectories(in dir: URL) throws -> [URL] {
        let entries = try FileManager.default.contentsOfDirectory(
            at: dir,
            includingPropertiesForKeys: [.isDirectoryKey],
            options: [.skipsHiddenFiles]
        )
        let filtered = entries.filter { url in
            guard url.lastPathComponent.hasPrefix("turn-") else { return false }
            let values = try? url.resourceValues(forKeys: [.isDirectoryKey])
            return values?.isDirectory == true
        }
        return filtered.sorted { $0.lastPathComponent < $1.lastPathComponent }
    }

    /// Parse `action.json`. Tolerates unknown extra fields; requires only
    /// a string `tool` and an object (or missing) `arguments`. Returns
    /// nil on I/O or schema failure.
    private static func parseActionJSON(at url: URL) -> ParsedAction? {
        guard let data = try? Data(contentsOf: url) else { return nil }
        guard
            let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
            let toolName = object["tool"] as? String,
            !toolName.isEmpty
        else {
            return nil
        }

        let argumentsValue: [String: Value]?
        if let argsDict = object["arguments"] as? [String: Any] {
            // Re-serialize the arguments subtree and decode it as
            // [String: Value] — matches the CallCommand JSON path and
            // hands us an argument map the ToolRegistry.call already
            // knows how to dispatch.
            do {
                let argData = try JSONSerialization.data(
                    withJSONObject: argsDict, options: []
                )
                argumentsValue = try JSONDecoder().decode(
                    [String: Value].self, from: argData
                )
            } catch {
                return nil
            }
        } else {
            argumentsValue = nil
        }

        return ParsedAction(toolName: toolName, arguments: argumentsValue)
    }

    private static func firstTextContent(_ content: [Tool.Content]) -> String? {
        for item in content {
            if case let .text(text, _, _) = item {
                return text
            }
        }
        return nil
    }

    private static func errorResult(_ message: String) -> CallTool.Result {
        CallTool.Result(
            content: [.text(text: message, annotations: nil, _meta: nil)],
            isError: true
        )
    }
}
