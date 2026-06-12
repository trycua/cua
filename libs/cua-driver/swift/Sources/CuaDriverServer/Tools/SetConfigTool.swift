import CoreGraphics
import CuaDriverCore
import Foundation
import MCP

/// Write a single setting into the persistent driver config at
/// `~/Library/Application Support/<app-name>/config.json`. Loads the
/// current config, applies the change, and atomically writes it back
/// through `ConfigStore.mutate`.
///
/// Keys use dotted snake_case paths so callers don't need per-field
/// tool names:
///   - `agent_cursor.enabled` → bool
///   - `agent_cursor.motion.start_handle|end_handle|arc_size|arc_flow|spring`
///     → number
///   - `capture_mode` → string (`vision` | `ax` | `som`)
///
/// Only leaf keys are settable — `agent_cursor` or `agent_cursor.motion`
/// without a sub-field is rejected so callers can't accidentally
/// overwrite a whole subtree by passing `null` or a malformed object.
/// Agent-cursor mutations are additionally propagated to
/// `AgentCursor.shared` so the live session reflects the new value
/// without a daemon restart; `capture_mode` has no live-state mirror
/// because `get_window_state` re-reads the persisted value on each
/// invocation.
public enum SetConfigTool {
    public static let handler = ToolHandler(
        tool: Tool(
            name: "set_config",
            description: """
                Write a single setting into the persistent driver config
                at `~/Library/Application Support/<app-name>/config.json`.
                Values survive daemon restarts AND are propagated to the
                live session state.

                Keys are dotted snake_case paths:
                  - `agent_cursor.enabled` (bool)
                  - `agent_cursor.motion.start_handle` (number)
                  - `agent_cursor.motion.end_handle` (number)
                  - `agent_cursor.motion.arc_size` (number)
                  - `agent_cursor.motion.arc_flow` (number)
                  - `agent_cursor.motion.spring` (number)
                  - `capture_mode` (string: `vision` | `ax` | `som`)

                Arguments: `{"key": "<dotted.path>", "value": <json>}`.
                """,
            inputSchema: [
                "type": "object",
                "required": ["key", "value"],
                "properties": [
                    "key": [
                        "type": "string",
                        "description":
                            "Dotted snake_case path to a leaf config field, e.g. `agent_cursor.enabled`.",
                    ],
                    "value": [
                        "description":
                            "New value for `key`. JSON type depends on the key.",
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
            guard let key = arguments?["key"]?.stringValue, !key.isEmpty else {
                return errorResult("Missing required string field `key`.")
            }
            guard let value = arguments?["value"] else {
                return errorResult("Missing required field `value`.")
            }

            do {
                try await applyConfigKey(key, value: value)
            } catch let err as ConfigKeyError {
                return errorResult(err.message)
            } catch {
                return errorResult(
                    "Failed to write config: \(error.localizedDescription)"
                )
            }

            let updated = await ConfigStore.shared.load()
            let encoder = ConfigStore.makeEncoder()
            let prettyText: String
            if let data = try? encoder.encode(updated),
                let str = String(data: data, encoding: .utf8)
            {
                prettyText = str
            } else {
                prettyText = "config: (encode failed)"
            }
            return CallTool.Result(
                content: [.text(text: "✅ \(prettyText)", annotations: nil, _meta: nil)]
            )
        }
    )

    private static func errorResult(_ message: String) -> CallTool.Result {
        CallTool.Result(
            content: [.text(text: message, annotations: nil, _meta: nil)],
            isError: true
        )
    }
}

/// Errors specific to `set_config` dispatch — unknown keys and type
/// mismatches surface as user-visible error results rather than
/// uncaught throws. Wrapping lets callers distinguish "user passed a
/// bad key/value" from "filesystem write failed" in the catch block.
public struct ConfigKeyError: Error {
    public let message: String
    public init(message: String) {
        self.message = message
    }
}

/// Apply a single dotted-path mutation to the persistent config AND to
/// the live `AgentCursor.shared` state. Exposed at module scope (not
/// nested inside `SetConfigTool`) so the CLI's in-process `config set`
/// path can call it directly without spinning up a tool handler.
public func applyConfigKey(_ key: String, value: Value) async throws {
    // Subtree-not-leaf guard first so the caller gets a targeted hint
    // rather than a generic "unknown key" when they strip the leaf by
    // accident (`cua-driver config set agent_cursor '{"enabled": false}'`).
    if key == "agent_cursor" || key == "agent_cursor.motion" {
        throw ConfigKeyError(
            message:
                "`\(key)` is a subtree, not a leaf — set a leaf like `agent_cursor.enabled` or `agent_cursor.motion.arc_size`."
        )
    }

    if key == "agent_cursor.enabled" {
        guard let flag = value.boolValue else {
            throw ConfigKeyError(
                message:
                    "`agent_cursor.enabled` expects a boolean; got \(describe(value))."
            )
        }
        try await ConfigStore.shared.mutate { config in
            config.agentCursor.enabled = flag
        }
        await MainActor.run {
            AgentCursor.shared.setEnabled(flag)
        }
        return
    }

    if key == "max_image_dimension" {
        guard let intVal = value.intValue, intVal >= 0 else {
            throw ConfigKeyError(
                message:
                    "`max_image_dimension` expects a non-negative integer; got \(describe(value))."
            )
        }
        try await ConfigStore.shared.mutate { config in
            config.maxImageDimension = intVal
        }
        return
    }

    if key == "capture_mode" {
        guard let raw = value.stringValue else {
            throw ConfigKeyError(
                message:
                    "`capture_mode` expects a string (`vision`, `ax`, or `som`); got \(describe(value))."
            )
        }
        // Accept `"screenshot"` as a deprecated alias for `"vision"`
        // — matches the back-compat decode in `CaptureMode.init(from:)`
        // so the CLI / MCP write path stays in sync with on-disk reads.
        let resolved = (raw == "screenshot") ? "vision" : raw
        guard let mode = CaptureMode(rawValue: resolved) else {
            let allowed =
                CaptureMode.allCases.map { $0.rawValue }.joined(separator: ", ")
            throw ConfigKeyError(
                message:
                    "`capture_mode` must be one of: \(allowed). Got `\(raw)`."
            )
        }
        try await ConfigStore.shared.mutate { config in
            config.captureMode = mode
        }
        // No live-state mirror — `GetAppStateTool` re-reads the
        // persisted value on every invocation, so the next call picks
        // up the new mode without a daemon bounce or extra wiring.
        return
    }

    if let knob = motionLeafKnob(for: key) {
        guard let number = numberValue(value) else {
            throw ConfigKeyError(
                message:
                    "`agent_cursor.motion.\(knob)` expects a number; got \(describe(value))."
            )
        }
        try await ConfigStore.shared.mutate { config in
            switch knob {
            case "start_handle": config.agentCursor.motion.startHandle = number
            case "end_handle": config.agentCursor.motion.endHandle = number
            case "arc_size": config.agentCursor.motion.arcSize = number
            case "arc_flow": config.agentCursor.motion.arcFlow = number
            case "spring": config.agentCursor.motion.spring = number
            default: break  // unreachable: guarded by `motionLeafKnob`
            }
        }
        await MainActor.run {
            var opts = AgentCursor.shared.defaultMotionOptions
            switch knob {
            case "start_handle": opts.startHandle = CGFloat(number)
            case "end_handle": opts.endHandle = CGFloat(number)
            case "arc_size": opts.arcSize = CGFloat(number)
            case "arc_flow": opts.arcFlow = CGFloat(number)
            case "spring": opts.spring = CGFloat(number)
            default: break
            }
            AgentCursor.shared.defaultMotionOptions = opts
        }
        return
    }

    throw ConfigKeyError(
        message:
            "Unknown config key `\(key)`. Supported: `agent_cursor.enabled`, `agent_cursor.motion.{start_handle,end_handle,arc_size,arc_flow,spring}`, `capture_mode`, `max_image_dimension`."
    )
}

/// If `key` is a fully-qualified motion leaf path (e.g.
/// `agent_cursor.motion.arc_size`), return the leaf knob name
/// (`arc_size`). Otherwise nil.
private func motionLeafKnob(for key: String) -> String? {
    let prefix = "agent_cursor.motion."
    guard key.hasPrefix(prefix) else { return nil }
    let knob = String(key.dropFirst(prefix.count))
    guard motionKnobs.contains(knob) else { return nil }
    return knob
}

private let motionKnobs: Set<String> = [
    "start_handle", "end_handle", "arc_size", "arc_flow", "spring",
]

/// MCP `Value` normalization for numeric knobs. JSON integer literals
/// decode as `.int` from the MCP bridge, so a bare `0` would slip past
/// a raw `doubleValue` check. Accept both forms.
private func numberValue(_ value: Value) -> Double? {
    if let d = value.doubleValue { return d }
    if let i = value.intValue { return Double(i) }
    return nil
}

private func describe(_ value: Value) -> String {
    switch value {
    case .null: return "null"
    case .bool: return "bool"
    case .int: return "int"
    case .double: return "number"
    case .string: return "string"
    case .data: return "binary"
    case .array: return "array"
    case .object: return "object"
    }
}
