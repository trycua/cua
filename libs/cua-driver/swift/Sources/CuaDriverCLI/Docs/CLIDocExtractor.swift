import CuaDriverCore
import Foundation

// MARK: - Documentation Types

/// Represents documentation for a single CLI command
struct CommandDoc: Codable {
    let name: String
    let abstract: String
    let discussion: String?
    let arguments: [ArgumentDoc]
    let options: [OptionDoc]
    let flags: [FlagDoc]
    let subcommands: [CommandDoc]
}

/// Represents documentation for a command argument
struct ArgumentDoc: Codable {
    let name: String
    let help: String
    let type: String
    let isOptional: Bool
}

/// Represents documentation for a command option
struct OptionDoc: Codable {
    let name: String
    let shortName: String?
    let help: String
    let type: String
    let defaultValue: String?
    let isOptional: Bool
}

/// Represents documentation for a command flag
struct FlagDoc: Codable {
    let name: String
    let shortName: String?
    let help: String
    let defaultValue: Bool
}

/// Root documentation structure
struct CLIDocumentation: Codable {
    let name: String
    let version: String
    let abstract: String
    let commands: [CommandDoc]
}

// MARK: - CLI Documentation Extractor

/// Extracts CLI documentation from command definitions.
/// This uses a combination of ArgumentParser's configuration metadata
/// and manually maintained help text to provide accurate documentation.
enum CLIDocExtractor {
    /// Extract documentation from all registered commands
    static func extractAll() -> CLIDocumentation {
        return CLIDocumentation(
            name: "cua-driver",
            version: CuaDriverCore.version,
            abstract: "macOS Accessibility-driven computer-use agent — MCP stdio server.",
            commands: allCommandDocs
        )
    }

    // MARK: - Command Documentation Definitions

    private static var allCommandDocs: [CommandDoc] {
        return [
            mcpDoc,
            callDoc,
            listToolsDoc,
            describeDoc,
            serveDoc,
            stopDoc,
            statusDoc,
            recordingDoc,
            configDoc,
            mcpConfigDoc,
            updateDoc,
            diagnoseDoc,
            doctorDoc,
            dumpDocsDoc,
        ]
    }

    // MARK: - mcp

    private static var mcpDoc: CommandDoc {
        CommandDoc(
            name: "mcp",
            abstract: "Run the stdio MCP server.",
            discussion: """
                When invoked from a shell or IDE terminal (Claude Code, Cursor,
                VS Code, Warp), macOS TCC attributes the process to the parent
                terminal — not to CuaDriver.app — so AX probes silently fail
                against the wrong bundle id. To sidestep this without breaking
                the stdio MCP transport, `mcp` detects the context, ensures a
                `cua-driver serve` daemon is running under LaunchServices
                (relaunching via `open -n -g -a CuaDriver --args serve` if not),
                and proxies every MCP tool call through the daemon's Unix
                socket. Tool semantics are identical to the in-process path.
                Pass `--no-daemon-relaunch` (or set CUA_DRIVER_MCP_NO_RELAUNCH=1)
                to force in-process execution — useful when the calling context
                already has the right TCC grants (e.g. spawned from CuaDriver.app
                directly), or for diagnosing in-process failures.
                """,
            arguments: [],
            options: [
                OptionDoc(name: "socket", shortName: nil, help: "Override the daemon Unix socket path used by the proxy fallback.", type: "String", defaultValue: nil, isOptional: true),
            ],
            flags: [
                FlagDoc(name: "claude-code-computer-use-compat", shortName: nil, help: "Expose normal CuaDriver tools, replacing only `screenshot` with a Claude Code-friendly window-only screenshot that establishes the vision coordinate frame.", defaultValue: false),
                FlagDoc(name: "no-daemon-relaunch", shortName: nil, help: "Stay in the current process instead of auto-launching a daemon and proxying through its Unix socket when invoked from a shell without CuaDriver.app's TCC grants. Also toggleable via CUA_DRIVER_MCP_NO_RELAUNCH=1.", defaultValue: false),
            ],
            subcommands: []
        )
    }

    // MARK: - call

    private static var callDoc: CommandDoc {
        CommandDoc(
            name: "call",
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
                """,
            arguments: [
                ArgumentDoc(name: "tool-name", help: "Name of the tool to invoke (see `cua-driver list-tools`).", type: "String", isOptional: false),
                ArgumentDoc(name: "json-args", help: "JSON object string for the tool's inputSchema. If omitted, reads from stdin when stdin is a pipe.", type: "String", isOptional: true),
            ],
            options: [
                OptionDoc(name: "screenshot-out-file", shortName: nil, help: "Write the first image content block from the response to this file path.", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "socket", shortName: nil, help: "Override the daemon Unix socket path.", type: "String", defaultValue: nil, isOptional: true),
            ],
            flags: [
                FlagDoc(name: "raw", shortName: nil, help: "Print the raw CallTool.Result JSON (content + structuredContent + isError) instead of unwrapping structuredContent.", defaultValue: false),
                FlagDoc(name: "compact", shortName: nil, help: "Emit minified JSON instead of pretty-printed.", defaultValue: false),
                FlagDoc(name: "no-daemon", shortName: nil, help: "Skip the cua-driver daemon even if one is running.", defaultValue: false),
            ],
            subcommands: []
        )
    }

    // MARK: - list-tools

    private static var listToolsDoc: CommandDoc {
        CommandDoc(
            name: "list-tools",
            abstract: "List every tool exposed by cua-driver with its one-line description.",
            discussion: nil,
            arguments: [],
            options: [
                OptionDoc(name: "socket", shortName: nil, help: "Override the daemon Unix socket path.", type: "String", defaultValue: nil, isOptional: true),
            ],
            flags: [
                FlagDoc(name: "no-daemon", shortName: nil, help: "Skip the cua-driver daemon even if one is running.", defaultValue: false),
            ],
            subcommands: []
        )
    }

    // MARK: - describe

    private static var describeDoc: CommandDoc {
        CommandDoc(
            name: "describe",
            abstract: "Print a tool's full description and JSON input schema.",
            discussion: nil,
            arguments: [
                ArgumentDoc(name: "tool-name", help: "Name of the tool to describe.", type: "String", isOptional: false),
            ],
            options: [
                OptionDoc(name: "socket", shortName: nil, help: "Override the daemon Unix socket path.", type: "String", defaultValue: nil, isOptional: true),
            ],
            flags: [
                FlagDoc(name: "compact", shortName: nil, help: "Emit minified JSON instead of pretty-printed.", defaultValue: false),
                FlagDoc(name: "no-daemon", shortName: nil, help: "Skip the cua-driver daemon even if one is running.", defaultValue: false),
            ],
            subcommands: []
        )
    }

    // MARK: - serve

    private static var serveDoc: CommandDoc {
        CommandDoc(
            name: "serve",
            abstract: "Run cua-driver as a long-running daemon on a Unix domain socket.",
            discussion: """
                Listens on --socket (default: ~/Library/Caches/cua-driver/cua-driver.sock).
                Subsequent `cua-driver call/list-tools/describe` invocations auto-detect
                the socket and forward their requests, so the AppStateEngine's per-pid
                element_index cache survives across CLI calls.
                """,
            arguments: [],
            options: [
                OptionDoc(name: "socket", shortName: nil, help: "Override the Unix socket path.", type: "String", defaultValue: nil, isOptional: true),
            ],
            flags: [
                FlagDoc(name: "no-relaunch", shortName: nil, help: "Stay in the current process instead of re-execing via `open -n -g -a CuaDriver`.", defaultValue: false),
            ],
            subcommands: []
        )
    }

    // MARK: - stop

    private static var stopDoc: CommandDoc {
        CommandDoc(
            name: "stop",
            abstract: "Ask the running cua-driver daemon to exit gracefully.",
            discussion: nil,
            arguments: [],
            options: [
                OptionDoc(name: "socket", shortName: nil, help: "Override the Unix socket path.", type: "String", defaultValue: nil, isOptional: true),
            ],
            flags: [],
            subcommands: []
        )
    }

    // MARK: - status

    private static var statusDoc: CommandDoc {
        CommandDoc(
            name: "status",
            abstract: "Report whether a cua-driver daemon is running.",
            discussion: nil,
            arguments: [],
            options: [
                OptionDoc(name: "socket", shortName: nil, help: "Override the Unix socket path.", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "pid-file", shortName: nil, help: "Override the daemon pid-file path.", type: "String", defaultValue: nil, isOptional: true),
            ],
            flags: [],
            subcommands: []
        )
    }

    // MARK: - recording

    private static var recordingDoc: CommandDoc {
        CommandDoc(
            name: "recording",
            abstract: "Control the trajectory recorder on a running cua-driver daemon.",
            discussion: """
                Wraps the `set_recording` and `get_recording_state` tools
                with human-friendly output. All subcommands require a
                running daemon (`cua-driver serve`) because recording
                state lives in-process and doesn't survive CLI-process
                lifetimes.
                """,
            arguments: [],
            options: [],
            flags: [],
            subcommands: [
                CommandDoc(
                    name: "start",
                    abstract: "Enable trajectory recording to the given directory.",
                    discussion: nil,
                    arguments: [
                        ArgumentDoc(name: "output-dir", help: "Directory to write turn folders into. Expands `~`; created if missing.", type: "String", isOptional: false),
                    ],
                    options: [
                        OptionDoc(name: "socket", shortName: nil, help: "Override the daemon Unix socket path.", type: "String", defaultValue: nil, isOptional: true),
                    ],
                    flags: [
                        FlagDoc(name: "video-experimental", shortName: nil, help: "Also capture the main display to `<output-dir>/recording.mp4` via SCStream. Experimental, off by default.", defaultValue: false),
                    ],
                    subcommands: []
                ),
                CommandDoc(
                    name: "stop",
                    abstract: "Disable trajectory recording.",
                    discussion: nil,
                    arguments: [],
                    options: [
                        OptionDoc(name: "socket", shortName: nil, help: "Override the daemon Unix socket path.", type: "String", defaultValue: nil, isOptional: true),
                    ],
                    flags: [],
                    subcommands: []
                ),
                CommandDoc(
                    name: "status",
                    abstract: "Report whether recording is currently enabled.",
                    discussion: nil,
                    arguments: [],
                    options: [
                        OptionDoc(name: "socket", shortName: nil, help: "Override the daemon Unix socket path.", type: "String", defaultValue: nil, isOptional: true),
                    ],
                    flags: [],
                    subcommands: []
                ),
                CommandDoc(
                    name: "render",
                    abstract: "Render a captured recording directory to a zoomed MP4.",
                    discussion: """
                        Reads `<input-dir>/session.json`, `<input-dir>/recording.mp4`,
                        `<input-dir>/cursor.jsonl`, and every `<input-dir>/turn-*/action.json`,
                        then produces a single zoom-on-click MP4 at `--output`.
                        """,
                    arguments: [
                        ArgumentDoc(name: "input-dir", help: "Recording directory (contains session.json + recording.mp4 + cursor.jsonl + turn-*/).", type: "String", isOptional: false),
                    ],
                    options: [
                        OptionDoc(name: "output", shortName: nil, help: "Destination path for the rendered MP4. Overwrites any existing file.", type: "String", defaultValue: nil, isOptional: false),
                        OptionDoc(name: "scale", shortName: nil, help: "Zoom factor applied to each click event. 1.0 disables zoom; 2.0 is 2× magnification.", type: "Double", defaultValue: "2.0", isOptional: true),
                    ],
                    flags: [
                        FlagDoc(name: "no-zoom", shortName: nil, help: "Skip the zoom curve and re-encode the input as-is. Useful as a baseline check.", defaultValue: false),
                    ],
                    subcommands: []
                ),
            ]
        )
    }

    // MARK: - config

    private static var configDoc: CommandDoc {
        CommandDoc(
            name: "config",
            abstract: "Read and write persistent cua-driver settings.",
            discussion: """
                Persistent config lives at
                `~/Library/Application Support/<app-name>/config.json` and
                survives daemon restarts.

                Examples:
                  cua-driver config                                    # print full config
                  cua-driver config get agent_cursor.enabled
                  cua-driver config set agent_cursor.enabled false
                  cua-driver config set agent_cursor.motion.arc_size 0.4
                  cua-driver config reset                              # overwrite with defaults
                """,
            arguments: [],
            options: [],
            flags: [],
            subcommands: [
                CommandDoc(
                    name: "show",
                    abstract: "Print the full current config as JSON.",
                    discussion: nil,
                    arguments: [],
                    options: [],
                    flags: [],
                    subcommands: []
                ),
                CommandDoc(
                    name: "get",
                    abstract: "Print a single config value by key.",
                    discussion: nil,
                    arguments: [
                        ArgumentDoc(name: "key", help: "Config key to read (e.g. `schema_version`, `agent_cursor.enabled`).", type: "String", isOptional: false),
                    ],
                    options: [],
                    flags: [],
                    subcommands: []
                ),
                CommandDoc(
                    name: "set",
                    abstract: "Write a single config value by dotted snake_case key.",
                    discussion: nil,
                    arguments: [
                        ArgumentDoc(name: "key", help: "Dotted snake_case path, e.g. `agent_cursor.enabled` or `agent_cursor.motion.arc_size`.", type: "String", isOptional: false),
                        ArgumentDoc(name: "value", help: "New value. JSON type depends on the key.", type: "String", isOptional: false),
                    ],
                    options: [
                        OptionDoc(name: "socket", shortName: nil, help: "Override the daemon Unix socket path.", type: "String", defaultValue: nil, isOptional: true),
                    ],
                    flags: [],
                    subcommands: []
                ),
                CommandDoc(
                    name: "reset",
                    abstract: "Overwrite the config file with defaults.",
                    discussion: nil,
                    arguments: [],
                    options: [],
                    flags: [],
                    subcommands: []
                ),
                CommandDoc(
                    name: "telemetry",
                    abstract: "Manage anonymous telemetry settings.",
                    discussion: nil,
                    arguments: [],
                    options: [],
                    flags: [],
                    subcommands: [
                        CommandDoc(name: "status", abstract: "Show whether anonymous telemetry is enabled.", discussion: nil, arguments: [], options: [], flags: [], subcommands: []),
                        CommandDoc(name: "enable", abstract: "Enable anonymous telemetry.", discussion: nil, arguments: [], options: [], flags: [], subcommands: []),
                        CommandDoc(name: "disable", abstract: "Disable anonymous telemetry.", discussion: nil, arguments: [], options: [], flags: [], subcommands: []),
                    ]
                ),
                CommandDoc(
                    name: "updates",
                    abstract: "Manage automatic update settings.",
                    discussion: nil,
                    arguments: [],
                    options: [],
                    flags: [],
                    subcommands: [
                        CommandDoc(name: "status", abstract: "Show whether automatic updates are enabled.", discussion: nil, arguments: [], options: [], flags: [], subcommands: []),
                        CommandDoc(name: "enable", abstract: "Enable automatic updates.", discussion: nil, arguments: [], options: [], flags: [], subcommands: []),
                        CommandDoc(name: "disable", abstract: "Disable automatic updates.", discussion: nil, arguments: [], options: [], flags: [], subcommands: []),
                    ]
                ),
            ]
        )
    }

    // MARK: - mcp-config

    private static var mcpConfigDoc: CommandDoc {
        CommandDoc(
            name: "mcp-config",
            abstract: "Print MCP server config or a client-specific install command.",
            discussion: nil,
            arguments: [],
            options: [
                OptionDoc(name: "client", shortName: nil, help: "Client to print the install command for: claude | codex | cursor | openclaw | opencode | hermes | pi. Omit for the generic JSON snippet.", type: "String", defaultValue: nil, isOptional: true),
            ],
            flags: [
                FlagDoc(name: "claude-code-computer-use-compat", shortName: nil, help: "Print config for Claude Code's window-scoped screenshot compatibility mode registered as `cua-computer-use`.", defaultValue: false),
            ],
            subcommands: []
        )
    }

    // MARK: - update

    private static var updateDoc: CommandDoc {
        CommandDoc(
            name: "update",
            abstract: "Check for a newer cua-driver release and apply it.",
            discussion: nil,
            arguments: [],
            options: [],
            flags: [
                FlagDoc(name: "apply", shortName: nil, help: "Download and apply the update without prompting.", defaultValue: false),
            ],
            subcommands: []
        )
    }

    // MARK: - diagnose

    private static var diagnoseDoc: CommandDoc {
        CommandDoc(
            name: "diagnose",
            abstract: "Print a paste-able bundle-path / cdhash / TCC-status report for support.",
            discussion: nil,
            arguments: [],
            options: [],
            flags: [],
            subcommands: []
        )
    }

    // MARK: - doctor

    private static var doctorDoc: CommandDoc {
        CommandDoc(
            name: "doctor",
            abstract: "Clean up stale install bits left from older cua-driver versions.",
            discussion: nil,
            arguments: [],
            options: [],
            flags: [],
            subcommands: []
        )
    }

    // MARK: - dump-docs

    private static var dumpDocsDoc: CommandDoc {
        CommandDoc(
            name: "dump-docs",
            abstract: "Output CLI and MCP tool documentation as JSON for tooling and integrations.",
            discussion: nil,
            arguments: [],
            options: [
                OptionDoc(name: "type", shortName: nil, help: "Documentation type: cli, mcp, or all", type: "String", defaultValue: "all", isOptional: true),
            ],
            flags: [
                FlagDoc(name: "pretty", shortName: nil, help: "Pretty-print JSON output", defaultValue: false),
            ],
            subcommands: []
        )
    }
}
