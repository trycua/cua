import ArgumentParser
import CuaDriverCore
import CuaDriverServer
import Foundation
import MCP

/// `cua-driver config [get <key> | set <key> <value> | reset]` —
/// management interface for the persistent driver config at
/// `~/Library/Application Support/<app-name>/config.json`.
///
/// Mirrors the shape of the `recording` subcommand (management wrapper
/// over `get_config` / `set_config` MCP tools). Unlike `recording`,
/// config can also be read/written without a running daemon — the JSON
/// file is the source of truth, so one-shot reads go straight through
/// the `ConfigStore` actor. Writes still prefer the daemon path when
/// one is reachable so the live process sees the new values on its
/// next `ConfigStore.shared.load()`.
///
/// Keys are dotted snake_case paths:
///   - `schema_version`
///   - `capture_mode`
///   - `max_image_dimension`
///   - `agent_cursor.enabled`
///   - `agent_cursor.motion.{start_handle,end_handle,arc_size,arc_flow,spring}`
struct ConfigCommand: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "config",
        abstract: "Read and write persistent cua-driver settings.",
        discussion: """
            Persistent config lives at
            `~/Library/Application Support/<app-name>/config.json` and
            survives daemon restarts.

            Examples:
              cua-driver config                                    # print full config
              cua-driver config get capture_mode
              cua-driver config set capture_mode vision
              cua-driver config get agent_cursor.enabled
              cua-driver config set agent_cursor.enabled false
              cua-driver config set agent_cursor.motion.arc_size 0.4
              cua-driver config reset                              # overwrite with defaults
            """,
        subcommands: [
            ConfigShowCommand.self,
            ConfigGetCommand.self,
            ConfigSetCommand.self,
            ConfigResetCommand.self,
            ConfigTelemetryCommand.self,
            ConfigUpdatesCommand.self,
        ],
        defaultSubcommand: ConfigShowCommand.self
    )
}

/// `cua-driver config telemetry {status|enable|disable}` — dedicated
/// wrapper around the `telemetry_enabled` config key for discoverability.
/// Equivalent to `config set telemetry_enabled {true|false}` but with
/// friendlier output and an explicit env-override callout on `status`.
struct ConfigTelemetryCommand: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "telemetry",
        abstract: "Manage anonymous telemetry settings.",
        subcommands: [
            ConfigTelemetryStatusCommand.self,
            ConfigTelemetryEnableCommand.self,
            ConfigTelemetryDisableCommand.self,
        ],
        defaultSubcommand: ConfigTelemetryStatusCommand.self
    )
}

struct ConfigTelemetryStatusCommand: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "status",
        abstract: "Show whether anonymous telemetry is enabled."
    )

    func run() throws {
        let config = ConfigStore.loadSync()
        let envOverride = ProcessInfo.processInfo.environment["CUA_DRIVER_TELEMETRY_ENABLED"]
        print("Telemetry enabled: \(config.telemetryEnabled)")
        if let envValue = envOverride {
            let lower = envValue.lowercased()
            if ["0", "1", "true", "false", "yes", "no", "on", "off"].contains(lower) {
                print("  (currently overridden by CUA_DRIVER_TELEMETRY_ENABLED=\(envValue))")
            }
        }
        print("")
        print("Telemetry collects anonymous usage data to help improve Cua Driver.")
        print("No personal information, file paths, or command arguments are collected.")
    }
}

struct ConfigTelemetryEnableCommand: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "enable",
        abstract: "Enable anonymous telemetry."
    )

    func run() throws {
        try ConfigStore.setTelemetryEnabledSync(true)
        print("Telemetry enabled")
        print("Thank you for helping improve Cua Driver!")
    }
}

struct ConfigTelemetryDisableCommand: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "disable",
        abstract: "Disable anonymous telemetry."
    )

    func run() throws {
        try ConfigStore.setTelemetryEnabledSync(false)
        print("Telemetry disabled")
    }
}

/// `cua-driver config show` — print the full current config as
/// pretty-printed JSON. Also the default action when `cua-driver config`
/// is invoked with no subcommand.
struct ConfigShowCommand: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "show",
        abstract: "Print the full current config as JSON."
    )

    func run() async throws {
        let config = await ConfigStore.shared.load()
        try printConfigJSON(config)
    }
}

/// `cua-driver config get <key>` — print a single value from the
/// current config. Exits 64 (usage) on unknown keys. Accepts dotted
/// snake_case paths (`agent_cursor.motion.arc_size`), same keyset as
/// `config set`.
struct ConfigGetCommand: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "get",
        abstract: "Print a single config value by key."
    )

    @Argument(
        help:
            "Config key to read (e.g. `schema_version`, `agent_cursor.enabled`).")
    var key: String

    func run() async throws {
        let config = await ConfigStore.shared.load()
        switch key {
        case "schema_version":
            print(config.schemaVersion)
        case "capture_mode":
            print(config.captureMode.rawValue)
        case "max_image_dimension":
            print(config.maxImageDimension)
        case "agent_cursor.enabled":
            print(config.agentCursor.enabled)
        case "agent_cursor.motion.start_handle":
            print(config.agentCursor.motion.startHandle)
        case "agent_cursor.motion.end_handle":
            print(config.agentCursor.motion.endHandle)
        case "agent_cursor.motion.arc_size":
            print(config.agentCursor.motion.arcSize)
        case "agent_cursor.motion.arc_flow":
            print(config.agentCursor.motion.arcFlow)
        case "agent_cursor.motion.spring":
            print(config.agentCursor.motion.spring)
        default:
            printErr("Unknown config key: \(key)")
            throw ExitCode(64)
        }
    }
}

/// `cua-driver config set <key> <value>` — write a single value. When
/// a daemon is running on the default socket, forwards to `set_config`
/// so the live process observes the update immediately. Otherwise
/// writes directly through the shared `ConfigStore`.
///
/// Accepts dotted snake_case leaf paths:
///   - `agent_cursor.enabled` (bool)
///   - `agent_cursor.motion.{start_handle|end_handle|arc_size|arc_flow|spring}` (number)
struct ConfigSetCommand: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "set",
        abstract: "Write a single config value by dotted snake_case key."
    )

    @Argument(
        help:
            "Dotted snake_case path, e.g. `agent_cursor.enabled` or `agent_cursor.motion.arc_size`."
    )
    var key: String

    @Argument(help: "New value. JSON type depends on the key.")
    var value: String

    @Option(name: .long, help: "Override the daemon Unix socket path.")
    var socket: String?

    func run() async throws {
        // Route through the daemon when one's reachable so the live
        // process picks up the new value immediately — the `set_config`
        // tool both persists AND mutates `AgentCursor.shared`. Without
        // a daemon we still write; any subsequent `cua-driver serve`
        // applies it on boot.
        let socketPath = socket ?? DaemonPaths.defaultSocketPath()
        if DaemonClient.isDaemonListening(socketPath: socketPath) {
            try await forwardSet(key: key, value: value, socketPath: socketPath)
            return
        }

        // In-process path — parse the value and hand off to the same
        // dispatcher the MCP tool uses, so the CLI and tool share a
        // keyset and keep in sync when new keys are added.
        let parsedValue: Value
        do {
            parsedValue = try parseValueArgument(value)
        } catch {
            printErr("Failed to parse `value`: \(error.localizedDescription)")
            throw ExitCode(65)
        }
        do {
            try await applyConfigKey(key, value: parsedValue)
        } catch let err as ConfigKeyError {
            printErr(err.message)
            throw ExitCode(64)
        } catch {
            printErr("Failed to write config: \(error.localizedDescription)")
            throw ExitCode(1)
        }
        let updated = await ConfigStore.shared.load()
        try printConfigJSON(updated)
    }

    private func forwardSet(key: String, value: String, socketPath: String)
        async throws
    {
        let parsedValue: Value
        do {
            parsedValue = try parseValueArgument(value)
        } catch {
            printErr("Failed to parse `value`: \(error.localizedDescription)")
            throw ExitCode(65)
        }

        let args: [String: Value] = [
            "key": .string(key),
            "value": parsedValue,
        ]
        let request = DaemonRequest(
            method: "call", name: "set_config", args: args
        )
        switch DaemonClient.sendRequest(request, socketPath: socketPath) {
        case .ok(let response):
            if !response.ok {
                printErr(response.error ?? "set_config failed")
                throw ExitCode(response.exitCode ?? 1)
            }
            guard case .call(let result) = response.result else {
                printErr("daemon returned unexpected result kind for call")
                throw ExitCode(1)
            }
            if result.isError == true {
                printErr(firstTextContent(result.content) ?? "set_config failed")
                // 64 = usage (unknown key) — matches the in-process path.
                throw ExitCode(64)
            }
            // Success path — print the updated config for confirmation.
            let updated = await ConfigStore.shared.load()
            try printConfigJSON(updated)
        case .noDaemon:
            printErr(
                "cua-driver daemon disappeared — start it with `cua-driver serve &`."
            )
            throw ExitCode(1)
        case .error(let message):
            printErr("daemon error: \(message)")
            throw ExitCode(1)
        }
    }
}

/// `cua-driver config reset` — overwrite the on-disk config with
/// defaults. Intentional, explicit wipe; doesn't delete the file so
/// `cat config.json` still shows the current baseline afterwards.
struct ConfigResetCommand: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "reset",
        abstract: "Overwrite the config file with defaults."
    )

    func run() async throws {
        do {
            try await ConfigStore.shared.reset()
        } catch {
            printErr("Failed to reset config: \(error.localizedDescription)")
            throw ExitCode(1)
        }
        let config = await ConfigStore.shared.load()
        try printConfigJSON(config)
    }
}

// MARK: - Shared helpers

/// Render a `CuaDriverConfig` to stdout using the same JSON shape that
/// hits disk (snake_case keys, pretty-printed, sorted). Callers get an
/// identical view whether they read the file with `cat` or through the
/// CLI.
private func printConfigJSON(_ config: CuaDriverConfig) throws {
    let encoder = ConfigStore.makeEncoder()
    let data = try encoder.encode(config)
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data("\n".utf8))
}

/// Parse a CLI `value` argument into an MCP `Value`. Tries JSON first
/// (so `--value 42` becomes `.int(42)`, `--value true` becomes
/// `.bool(true)`, `--value '"foo"'` becomes `.string("foo")`). Falls
/// back to treating the raw arg as a string when JSON decode fails —
/// the common case is `cua-driver config set capture_mode window`, and
/// we don't want users to wrap unquoted strings in extra quotes.
private func parseValueArgument(_ raw: String) throws -> Value {
    let data = Data(raw.utf8)
    if let decoded = try? JSONDecoder().decode(Value.self, from: data) {
        return decoded
    }
    return .string(raw)
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

/// `cua-driver config updates {status|enable|disable}` — dedicated
/// wrapper around the `auto_update_enabled` config key for discoverability.
/// Equivalent to `config set auto_update_enabled {true|false}` but with
/// friendlier output and an explicit env-override callout on `status`.
struct ConfigUpdatesCommand: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "updates",
        abstract: "Manage automatic update settings.",
        subcommands: [
            ConfigUpdatesStatusCommand.self,
            ConfigUpdatesEnableCommand.self,
            ConfigUpdatesDisableCommand.self,
        ],
        defaultSubcommand: ConfigUpdatesStatusCommand.self
    )
}

struct ConfigUpdatesStatusCommand: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "status",
        abstract: "Show whether automatic updates are enabled."
    )

    func run() throws {
        let config = ConfigStore.loadSync()
        let envOverride = ProcessInfo.processInfo.environment["CUA_DRIVER_AUTO_UPDATE_ENABLED"]
        print("Auto-update enabled: \(config.autoUpdateEnabled)")
        if let envValue = envOverride {
            let lower = envValue.lowercased()
            if ["0", "1", "true", "false", "yes", "no", "on", "off"].contains(lower) {
                print("  (currently overridden by CUA_DRIVER_AUTO_UPDATE_ENABLED=\(envValue))")
            }
        }
        print("")
        print("When enabled, cua-driver automatically checks for updates periodically")
        print("and can download and install new versions without user interaction.")
    }
}

struct ConfigUpdatesEnableCommand: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "enable",
        abstract: "Enable automatic updates."
    )

    func run() throws {
        try ConfigStore.setAutoUpdateEnabledSync(true)
        print("Auto-update enabled")
        print("The update service will check for new versions at login and periodically.")
    }
}

struct ConfigUpdatesDisableCommand: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "disable",
        abstract: "Disable automatic updates."
    )

    func run() throws {
        try ConfigStore.setAutoUpdateEnabledSync(false)
        print("Auto-update disabled")
        print("The LaunchAgent will be removed from your system.")
        
        // Remove the LaunchAgent
        let plistPath = "\(NSHomeDirectory())/Library/LaunchAgents/com.trycua.cua_driver_updater.plist"
        let fileManager = FileManager.default
        if fileManager.fileExists(atPath: plistPath) {
            do {
                try fileManager.removeItem(atPath: plistPath)
                print("Removed LaunchAgent.")
            } catch {
                print("Note: Failed to remove LaunchAgent (you can do this manually):")
                print("  rm \(plistPath)")
            }
        }
    }
}
