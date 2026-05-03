import ArgumentParser
import Foundation

struct Setup: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        abstract: "[Preview] Run unattended Setup Assistant automation on a macOS VM",
        discussion: """
            This is an experimental feature. Two modes are available:

            - preset: Uses a YAML config file with scripted click/type commands (default).
              Presets are specific to macOS versions and may break across releases.

            - agent: Uses Claude's computer-use API to intelligently navigate the Setup Assistant.
              Adapts to any macOS version. Requires an Anthropic API key.

            The VM must be freshly created and not have completed the Setup Assistant yet.
            """
    )

    @Argument(
        help: "Name of the virtual machine",
        completion: .custom(completeVMName))
    var name: String

    @Option(
        name: .customLong("mode"),
        help: "Setup mode: 'preset' for YAML-based automation, 'agent' for Claude computer-use agent (default: preset)")
    var mode: String = "preset"

    @Option(
        name: .customLong("unattended"),
        help: "Preset name or path to YAML config file (required for preset mode). Built-in presets: sequoia, tahoe.",
        completion: .file(extensions: ["yml", "yaml"]))
    var unattended: String?

    @Option(
        name: .customLong("anthropic-key"),
        help: "Anthropic API key for agent mode. Can also be set via ANTHROPIC_API_KEY env var.")
    var anthropicKey: String?

    @Option(
        name: .customLong("model"),
        help: "Claude model to use for agent mode (default: claude-sonnet-4-6)")
    var model: String = "claude-sonnet-4-6"

    @Option(
        name: .customLong("max-iterations"),
        help: "Maximum agent iterations before giving up (default: 200)")
    var maxIterations: Int = 200

    @Option(
        name: .customLong("system-prompt"),
        help: "Custom system prompt for the agent (overrides the default)")
    var systemPrompt: String?

    @Option(
        name: .customLong("storage"),
        help: "VM storage location to use or direct path to VM location")
    var storage: String?

    @Option(
        name: .customLong("vnc-port"),
        help: "Port to use for the VNC server. Defaults to 0 (auto-assign)")
    var vncPort: Int = 0

    @Flag(
        name: .customLong("no-display"),
        help: "Do not open the VNC client automatically")
    var noDisplay: Bool = false

    @Flag(
        name: .customLong("debug"),
        help: "Enable debug mode - saves screenshots with click coordinates")
    var debug: Bool = false

    @Option(
        name: .customLong("debug-dir"),
        help: "Custom directory for debug screenshots (defaults to unique folder in system temp)",
        completion: .directory)
    var debugDir: String?

    init() {}

    @MainActor
    func run() async throws {
        Logger.info("[Preview] Unattended setup is an experimental feature")

        switch mode {
        case "agent":
            // Resolve API key from flag or environment
            let apiKey = anthropicKey ?? ProcessInfo.processInfo.environment["ANTHROPIC_API_KEY"]
            guard let apiKey, !apiKey.isEmpty else {
                throw UnattendedError.configLoadFailed(
                    "Agent mode requires an Anthropic API key. Provide --anthropic-key or set ANTHROPIC_API_KEY env var."
                )
            }

            TelemetryClient.shared.record(event: TelemetryEvent.setup, properties: [
                "mode": "agent",
                "model": model
            ])

            try await LumeController().setupWithAgent(
                name: name,
                apiKey: apiKey,
                model: model,
                maxIterations: maxIterations,
                systemPrompt: systemPrompt,
                storage: storage,
                vncPort: vncPort,
                noDisplay: noDisplay,
                debug: debug,
                debugDir: debugDir
            )

        case "preset":
            guard let unattended else {
                throw UnattendedError.configLoadFailed(
                    "Preset mode requires --unattended <preset|path>. Built-in presets: sequoia, tahoe."
                )
            }

            let config: UnattendedConfig
            let isPreset = UnattendedConfig.isPreset(name: unattended)
            do {
                config = try UnattendedConfig.load(from: unattended)
                Logger.info("Loaded unattended config", metadata: [
                    "source": isPreset ? "preset:\(unattended)" : unattended,
                    "bootWait": "\(config.bootWait)s",
                    "commands": "\(config.bootCommands.count)"
                ])
            } catch {
                throw UnattendedError.configLoadFailed("Failed to load config from \(unattended): \(error.localizedDescription)")
            }

            TelemetryClient.shared.record(event: TelemetryEvent.setup, properties: [
                "mode": "preset",
                "preset_name": isPreset ? unattended : "custom"
            ])

            try await LumeController().setup(
                name: name,
                config: config,
                storage: storage,
                vncPort: vncPort,
                noDisplay: noDisplay,
                debug: debug,
                debugDir: debugDir
            )

        default:
            throw UnattendedError.configLoadFailed("Unknown setup mode '\(mode)'. Use 'preset' or 'agent'.")
        }
    }
}
