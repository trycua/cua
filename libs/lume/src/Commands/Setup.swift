import ArgumentParser
import Foundation

struct Setup: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        abstract: "Prepare unattended macOS setup",
        discussion: """
            Lume prepares the macOS disk offline, skips Setup Assistant, enables
            autologin and SSH, disables screensaver lock, verifies SSH, then stops
            the VM.

            The VM must be stopped before setup can patch its disk.
            """
    )

    @Argument(
        help: "Name of the virtual machine",
        completion: .custom(completeVMName))
    var name: String

    @Option(
        name: .customLong("unattended"),
        help: "Defaults to tahoe. Preset name or YAML path for compatibility and optional post-SSH commands. Built-in presets: sequoia, tahoe.",
        completion: .file(extensions: ["yml", "yaml"]))
    var unattended: String?

    @Option(
        name: .customLong("storage"),
        help: "VM storage location to use or direct path to VM location")
    var storage: String?

    @Option(
        name: .customLong("vnc-port"),
        help: "Port to use for the temporary verification VNC server. Defaults to 0 (auto-assign)")
    var vncPort: Int = 0

    @Flag(
        name: .customLong("no-display"),
        help: "Compatibility flag; offline setup verifies headlessly")
    var noDisplay: Bool = false

    @Flag(
        name: .customLong("debug"),
        help: "Compatibility flag; ignored by offline setup")
    var debug: Bool = false

    @Option(
        name: .customLong("debug-dir"),
        help: "Compatibility option; ignored by offline setup",
        completion: .directory)
    var debugDir: String?

    init() {}

    @MainActor
    func run() async throws {
        let unattended = unattended ?? "tahoe"

        let config: UnattendedConfig
        let isPreset = UnattendedConfig.isPreset(name: unattended)
        do {
            config = try UnattendedConfig.load(from: unattended)
            Logger.info("Loaded unattended config", metadata: [
                "source": isPreset ? "preset:\(unattended)" : unattended,
                "bootWait": "\(config.bootWait)s",
                "config_boot_commands": "\(config.bootCommands.count)"
            ])
        } catch {
            throw UnattendedError.configLoadFailed("Failed to load config from \(unattended): \(error.localizedDescription)")
        }

        TelemetryClient.shared.record(event: TelemetryEvent.setup, properties: [
            "mode": "offline",
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
    }
}
