import ArgumentParser
import Foundation

struct Setup: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        abstract: "[Preview] Run unattended Setup Assistant automation on a macOS VM",
        discussion: """
            This is an experimental feature. Unattended configurations are specific to macOS \
            versions and may not work across different releases.

            The VM must be freshly created and not have completed the Setup Assistant yet.
            """
    )

    @Argument(
        help: "Name of the virtual machine",
        completion: .custom(completeVMName))
    var name: String

    @Option(
        name: .customLong("unattended"),
        help: "Preset name or path to YAML config file for unattended macOS Setup Assistant automation. Built-in presets: tahoe.",
        completion: .file(extensions: ["yml", "yaml"]))
    var unattended: String

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

        // Load unattended config
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

        try await LumeController().setup(
            name: name,
            config: config,
            storage: storage,
            vncPort: vncPort,
            noDisplay: noDisplay,
            debug: debug,
            debugDir: debugDir
        )
    }
}
