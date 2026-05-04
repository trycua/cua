import AppKit
import ArgumentParser
import CuaDriverCore
import CuaDriverServer
import Foundation
import MCP

struct CuaDriverCommand: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "cua-driver",
        abstract: "macOS Accessibility-driven computer-use agent — MCP stdio server.",
        version: CuaDriverCore.version,
        subcommands: [
            MCPCommand.self,
            CallCommand.self,
            ListToolsCommand.self,
            DescribeCommand.self,
            ServeCommand.self,
            StopCommand.self,
            StatusCommand.self,
            RecordingCommand.self,
            ConfigCommand.self,
            MCPConfigCommand.self,
            UpdateCommand.self,
            DiagnoseCommand.self,
            DoctorCommand.self,
            DumpDocsCommand.self,
        ]
    )
}

/// `cua-driver mcp-config` — print the JSON snippet that MCP clients
/// (Claude Code, Cursor, custom SDK clients) need to register
/// cua-driver as an MCP server. Paste into `~/.claude/mcp.json` (or
/// equivalent) and the client auto-spawns cua-driver on demand.
struct MCPConfigCommand: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "mcp-config",
        abstract: "Print MCP server config or a client-specific install command."
    )

    @Option(name: .customLong("client"),
            help: "Client to print the install command for: claude | codex | cursor | openclaw | opencode | hermes | pi. Omit for the generic JSON snippet.")
    var client: String?

    func run() throws {
        let binary = resolvedBinaryPath()
        switch client?.lowercased() {
        case nil, "":
            print(genericMcpServersSnippet(binary: binary, includeType: false))
        case "claude":
            print("claude mcp add --transport stdio cua-driver -- \(binary) mcp")
        case "codex":
            print("codex mcp add cua-driver -- \(binary) mcp")
        case "cursor":
            // Cursor has no CLI — emit JSON the user pastes into
            // ~/.cursor/mcp.json (global) or .cursor/mcp.json (project).
            print(genericMcpServersSnippet(binary: binary, includeType: true))
        case "openclaw":
            // OpenClaw has a CLI registry — set with a JSON arg.
            print("openclaw mcp set cua-driver '{\"command\":\"\(binary)\",\"args\":[\"mcp\"]}'")
        case "opencode":
            // OpenCode (sst/opencode) uses opencode.json with type:"local"
            // and command as a single merged array.
            let snippet = """
            // paste under "mcp" in opencode.json (or opencode.jsonc):
            {
              "$schema": "https://opencode.ai/config.json",
              "mcp": {
                "cua-driver": {
                  "type": "local",
                  "command": ["\(binary)", "mcp"],
                  "enabled": true
                }
              }
            }
            """
            print(snippet)
        case "hermes":
            // Hermes (NousResearch) — YAML at ~/.hermes/config.yaml.
            // Reload inside Hermes with /reload-mcp after editing.
            let snippet = """
            # paste under mcp_servers in ~/.hermes/config.yaml,
            # then run /reload-mcp inside Hermes:
            mcp_servers:
              cua-driver:
                command: "\(binary)"
                args: ["mcp"]
            """
            print(snippet)
        case "pi":
            // Pi (badlogic/pi-mono) intentionally rejects MCP. Skip MCP and
            // point at the shell-tool path — Pi can shell-out to cua-driver
            // directly the same way it would call any other CLI tool.
            print("""
            Pi (badlogic/pi-mono) does not support MCP natively — the author
            has stated MCP support will not be added for context-budget reasons.

            Use cua-driver as a plain CLI from inside Pi instead:

                \(binary) list_apps
                \(binary) click  '{"pid": 1234, "x": 100, "y": 200}'
                \(binary) --help        # full tool catalog

            Each call is one-shot and returns JSON / text on stdout, which is
            exactly the shape Pi is designed around.

            Community MCP shims also exist if you really need MCP semantics
            (0xKobold/pi-mcp, nicobailon/pi-mcp-adapter) — these are not
            supported by us.
            """)
        default:
            FileHandle.standardError.write(Data(
                ("Unknown client '\(client!)'. Valid: claude, codex, cursor, openclaw, opencode, hermes, pi.\n").utf8
            ))
            throw ExitCode(2)
        }
    }

    private func genericMcpServersSnippet(binary: String, includeType: Bool) -> String {
        let typeLine = includeType ? ",\n      \"type\": \"stdio\"" : ""
        return """
        {
          "mcpServers": {
            "cua-driver": {
              "command": "\(binary)",
              "args": ["mcp"]\(typeLine)
            }
          }
        }
        """
    }

    private func resolvedBinaryPath() -> String {
        // `Bundle.main.executablePath` points at the physical binary
        // inside the .app bundle even when invoked via a symlink. Falls
        // back to argv[0] for raw `swift run` contexts.
        if let path = Bundle.main.executablePath {
            return path
        }
        return CommandLine.arguments.first ?? "cua-driver"
    }
}

/// Top-level entry point. Before handing to ArgumentParser, rewrite
/// argv so unknown first positional args dispatch to `call`:
///
///     cua-driver list_apps                   →  cua-driver call list_apps
///     cua-driver launch_app '{...}'          →  cua-driver call launch_app '{...}'
///     cua-driver get_window_state '{"pid":844,"window_id":1234}'
///
/// Known subcommands (`mcp`, `serve`, `stop`, `status`, `list-tools`,
/// `describe`, `call`, `help`) and any flag-prefixed arg stay untouched.
///
/// The collision rule is: tool names are `snake_case` (underscores),
/// subcommand names are `kebab-case` (hyphens). Different separators
/// mean no ambiguity — we can tell them apart at argv inspection time.
@main
struct CuaDriverEntryPoint {
    // Management subcommands that MUST NOT be rewritten to `call`.
    // Keep in sync with `CuaDriverCommand.configuration.subcommands`
    // plus the implicit `help`, `--help`, `-h`, `--version`, `-v`.
    private static let managementSubcommands: Set<String> = [
        "mcp",
        "mcp-config",
        "call",
        "list-tools",
        "describe",
        "serve",
        "stop",
        "status",
        "recording",
        "config",
        "update",
        "diagnose",
        "doctor",
        "dump-docs",
        "help",
    ]

    static func main() async {
        let original = Array(CommandLine.arguments.dropFirst())

        // First-run installation ping. Fires at most once per install
        // (guarded by a marker file under ~/.cua-driver/) and bypasses
        // the opt-out check so we can count adoption. Every subsequent
        // event honors the opt-out flag.
        TelemetryClient.shared.recordInstallation()

        // Per-entry-point event. Records which CLI surface (mcp /
        // serve / call / …) kicked off this process. Opt-out-respecting.
        let entryEvent = telemetryEntryEvent(for: original)
        TelemetryClient.shared.record(event: entryEvent)

        // Bare launch (no args) — typically a double-click from Finder
        // / Spotlight / Dock on CuaDriver.app. LSUIElement=true keeps
        // the binary headless by default, so without this branch a
        // DMG user sees "nothing happens" on open. Route through the
        // permissions gate instead: it's our one visible surface and
        // handles the "grant Accessibility + Screen Recording" flow
        // the user would otherwise have to discover on their own.
        if original.isEmpty {
            // NOTE: must be a synchronous call, not `await`. The
            // `await` on an async function creates a suspension
            // point; Swift's cooperative executor may resume on a
            // non-main thread, and NSApplication.shared.run() inside
            // runFirstLaunchGUI crashes when called off the main
            // thread (observed: EXC_BREAKPOINT in
            // NSUpdateCycleInitialize at `-[NSApplication run]`).
            // The MCP path works because MCPCommand.run is a sync
            // ParsableCommand method — the whole chain from main()
            // stays on the main thread.
            runFirstLaunchGUI()
            return
        }

        let rewritten = rewriteForImplicitCall(original)
        do {
            let parsed = try CuaDriverCommand.parseAsRoot(rewritten)
            if var asyncCommand = parsed as? AsyncParsableCommand {
                try await asyncCommand.run()
            } else {
                var syncCommand = parsed
                try syncCommand.run()
            }
        } catch {
            CuaDriverCommand.exit(withError: error)
        }
    }

    /// Bare-launch path — present the PermissionsGate window as the
    /// visible first-run UI. Terminates the process once the user
    /// completes the flow or closes the window. Shell / MCP-spawned
    /// invocations never reach this branch (they always have args).
    ///
    /// Deliberately synchronous: see the caller note in `main()` —
    /// `NSApplication.shared.run()` below (inside
    /// `runBlockingAppKitWith`) must be called on the main thread,
    /// and an `async` function call + `await` from async `main()`
    /// can resume on a cooperative executor thread.
    private static func runFirstLaunchGUI() {
        AppKitBootstrap.runBlockingAppKitWith {
            // Override AppKitBootstrap's default `.accessory` policy:
            // a bare-launch from Finder / Spotlight wants a Dock icon
            // so the user sees the app started AND the window can
            // grab focus. Shell / MCP subprocesses stay `.accessory`
            // (they never reach this path).
            await MainActor.run {
                NSApplication.shared.setActivationPolicy(.regular)
            }
            _ = await MainActor.run {
                PermissionsGate.shared
            }.ensureGranted(alwaysPresentWindow: true)
            // User either granted everything (green) or closed the
            // window. Either way the app's job is done for this
            // session; let AppKitBootstrap tear down and exit.
        }
    }

    /// Returns `args` unchanged when the first positional arg is a known
    /// subcommand, a flag, or absent. Otherwise prepends `call` so
    /// ArgumentParser routes the invocation through `CallCommand`.
    static func rewriteForImplicitCall(_ args: [String]) -> [String] {
        guard let first = args.first else { return args }
        if first.hasPrefix("-") { return args }  // flag — leave alone
        if managementSubcommands.contains(first) { return args }
        return ["call"] + args
    }

    /// Map the (pre-rewrite) argv to a telemetry event name. No argv
    /// values are ever included — just the subcommand name. `call`
    /// invocations report as `cua_driver_api_<tool>` so per-tool usage
    /// shows up in aggregate; everything else maps to a canonical
    /// `cua_driver_<subcommand>` event.
    static func telemetryEntryEvent(for args: [String]) -> String {
        guard let first = args.first else {
            return TelemetryEvent.guiLaunch
        }
        // `call <tool>` → per-tool event for adoption visibility.
        if first == "call", args.count >= 2 {
            return TelemetryEvent.apiPrefix + args[1]
        }
        // Implicit-call form — `cua-driver list_apps` rewrites to
        // `call list_apps` internally, so we check the same shape here
        // before fallback-mapping.
        if !first.hasPrefix("-") && !managementSubcommands.contains(first) {
            return TelemetryEvent.apiPrefix + first
        }
        switch first {
        case "mcp": return TelemetryEvent.mcp
        case "mcp-config": return "cua_driver_mcp_config"
        case "serve": return TelemetryEvent.serve
        case "stop": return TelemetryEvent.stop
        case "status": return TelemetryEvent.status
        case "list-tools": return TelemetryEvent.listTools
        case "describe": return TelemetryEvent.describe
        case "recording": return TelemetryEvent.recording
        case "config": return TelemetryEvent.config
        default: return TelemetryEvent.guiLaunch
        }
    }
}

struct MCPCommand: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "mcp",
        abstract: "Run the stdio MCP server."
    )

    func run() throws {
        // MCP stdio runs for the lifetime of the host process, so we
        // bootstrap AppKit here — the agent cursor overlay (disabled
        // by default, enabled via `set_agent_cursor_enabled`) needs a
        // live NSApplication event loop to draw. When the cursor's
        // never enabled, this costs us one idle run-loop.
        AppKitBootstrap.runBlockingAppKitWith {
            // Preflight TCC grants. When both are already active this
            // returns immediately; otherwise a small panel guides the
            // user through granting them and we resume once everything
            // flips green. User closing the panel without granting ->
            // exit with a clear message.
            let granted = await MainActor.run {
                PermissionsGate.shared
            }.ensureGranted()
            if !granted {
                FileHandle.standardError.write(
                    Data(
                        "cua-driver: required permissions (Accessibility + Screen Recording) not granted; MCP server exiting.\n"
                            .utf8))
                throw AppKitBootstrapError.permissionsDenied
            }

            // Same startup-warm as `serve`: surface any config decode
            // warnings on the host's stderr before the first tool call
            // hits the disk-read path.
            let config = await ConfigStore.shared.load()

            // Apply persisted agent-cursor preferences to the live
            // singleton so stdio MCP sessions also honor the user's
            // last-written state.
            await MainActor.run {
                AgentCursor.shared.apply(config: config.agentCursor)
            }

            let server = await CuaDriverMCPServer.make()
            let transport = StdioTransport()
            try await server.start(transport: transport)
            await server.waitUntilCompleted()
        }
    }
}

/// Bootstrap AppKit on the main thread so `AgentCursor` can draw its
/// overlay window + CA animations. The caller's async work runs on a
/// detached Task; the main thread blocks inside `NSApplication.run()`
/// and pumps AppKit events plus any GCD-main-queue dispatches Swift
/// concurrency uses to schedule `@MainActor` work. When the detached
/// work completes (or throws), we terminate AppKit so the process
/// exits cleanly.
///
/// Activation policy `.accessory` keeps the driver out of the Dock and
/// out of Cmd-Tab while still letting it own visible windows.
enum AppKitBootstrapError: Error, CustomStringConvertible {
    case permissionsDenied

    var description: String {
        switch self {
        case .permissionsDenied:
            return "permissions denied"
        }
    }
}

enum AppKitBootstrap {
    static func runBlockingAppKitWith(
        _ work: @Sendable @escaping () async throws -> Void
    ) {
        // Swift 6.1's strict-concurrency rejects direct calls to
        // `NSApplication.shared` / `setActivationPolicy` / `.run()`
        // from a nonisolated context. Callers are all CLI entry
        // points running on the main thread (they've already dropped
        // into synchronous `main()` or ArgumentParser's nonisolated
        // `run()` path), so we assert that with `MainActor.assumeIsolated`
        // rather than ripple `@MainActor` through every caller chain.
        MainActor.assumeIsolated {
            NSApplication.shared.setActivationPolicy(.accessory)

            Task.detached(priority: .userInitiated) {
                do {
                    try await work()
                } catch AppKitBootstrapError.permissionsDenied {
                    // Already logged by the caller; skip the generic
                    // "cua-driver: <error>" line to avoid duplicating.
                } catch {
                    FileHandle.standardError.write(
                        Data("cua-driver: \(error)\n".utf8)
                    )
                }
                await MainActor.run { NSApp.terminate(nil) }
            }

            NSApplication.shared.run()
        }
    }
}

/// `cua-driver update` — check for a newer release and optionally apply it.
struct UpdateCommand: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "update",
        abstract: "Check for a newer cua-driver release and apply it."
    )

    @Flag(name: .long, help: "Download and apply the update without prompting.")
    var apply = false

    func run() async throws {
        let current = CuaDriverCore.version
        print("Current version: \(current)")
        print("Checking for updates…")

        guard let latest = await VersionCheck.fetchLatest() else {
            print("Could not reach GitHub — check your connection and try again.")
            throw ExitCode(1)
        }

        guard VersionCheck.isNewer(latest, than: current) else {
            print("Already up to date.")
            return
        }

        print("New version available: \(latest)")

        if !apply {
            print("")
            print("Run with --apply to download and install it:")
            print("  cua-driver update --apply")
            print("")
            print("Or reinstall directly:")
            print("  curl -fsSL https://raw.githubusercontent.com/trycua/cua/main/libs/cua-driver/scripts/install.sh | bash")
            return
        }

        print("Downloading and installing cua-driver \(latest)…")
        let proc = Process()
        proc.executableURL = URL(fileURLWithPath: "/bin/bash")
        proc.arguments = ["-c",
            "curl -fsSL https://raw.githubusercontent.com/trycua/cua/main/libs/cua-driver/scripts/install.sh | bash"]
        try proc.run()
        proc.waitUntilExit()
        if proc.terminationStatus != 0 {
            print("Installation failed — run the command above manually for details.")
            throw ExitCode(Int32(proc.terminationStatus))
        }
    }
}

/// `cua-driver doctor` — clean up stale install bits left from older versions.
///
/// v0.0.5 and earlier installed a weekly LaunchAgent at
/// `~/Library/LaunchAgents/com.trycua.cua_driver_updater.plist` and a companion
/// `/usr/local/bin/cua-driver-update` script. v0.0.6 dropped both in favor of
/// the explicit `cua-driver update` command, but users who upgraded via the
/// legacy auto-updater path still have these dead files lingering.
///
/// Removing the LaunchAgent stops the weekly cron from firing the stale
/// update script. The plist lives under `$HOME` (no sudo). The companion
/// script under `/usr/local/bin` is root-owned, so we print the exact
/// `sudo rm` command for the user to run if it still exists.
struct DoctorCommand: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "doctor",
        abstract: "Clean up stale install bits left from older cua-driver versions."
    )

    func run() throws {
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        let legacyPlist = "\(home)/Library/LaunchAgents/com.trycua.cua_driver_updater.plist"
        let legacyScript = "/usr/local/bin/cua-driver-update"

        var removedCount = 0
        var manualSteps: [String] = []

        // LaunchAgent — no sudo needed, lives under $HOME.
        if FileManager.default.fileExists(atPath: legacyPlist) {
            // Best-effort unload before removal — tolerate failure since the
            // agent may not be loaded.
            let unload = Process()
            unload.executableURL = URL(fileURLWithPath: "/bin/launchctl")
            unload.arguments = ["unload", legacyPlist]
            unload.standardOutput = Pipe()
            unload.standardError = Pipe()
            try? unload.run()
            unload.waitUntilExit()

            do {
                try FileManager.default.removeItem(atPath: legacyPlist)
                print("✓ removed legacy LaunchAgent: \(legacyPlist)")
                removedCount += 1
            } catch {
                print("✗ could not remove \(legacyPlist): \(error)")
            }
        }

        // Update script — root-owned. Try without sudo first; on failure,
        // surface the exact command for the user to run manually.
        if FileManager.default.fileExists(atPath: legacyScript) {
            if FileManager.default.isWritableFile(atPath: legacyScript)
               && FileManager.default.isWritableFile(atPath: "/usr/local/bin")
            {
                do {
                    try FileManager.default.removeItem(atPath: legacyScript)
                    print("✓ removed legacy update script: \(legacyScript)")
                    removedCount += 1
                } catch {
                    manualSteps.append("sudo rm -f \(legacyScript)")
                }
            } else {
                manualSteps.append("sudo rm -f \(legacyScript)")
            }
        }

        if removedCount == 0 && manualSteps.isEmpty {
            print("Nothing to clean — install is up to date.")
            return
        }

        if !manualSteps.isEmpty {
            print("")
            print("The following needs to be removed manually (root-owned):")
            for step in manualSteps {
                print("  \(step)")
            }
        }
    }
}
