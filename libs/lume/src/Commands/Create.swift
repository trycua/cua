import ArgumentParser
import Foundation
import Virtualization

// MARK: - Create Command

struct Create: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        abstract: "Create a new virtual machine"
    )

    @Argument(help: "Name for the virtual machine")
    var name: String

    @Option(
        help: "Operating system to install. Defaults to macOS.",
        completion: .list(["macOS", "linux"]))
    var os: String = "macOS"

    @Option(help: "Number of CPU cores", transform: { Int($0) ?? 4 })
    var cpu: Int = 4

    @Option(
        help: "Memory size, e.g., 8192MB or 8GB. Defaults to 8GB.", transform: { try parseSize($0) }
    )
    var memory: UInt64 = 8 * 1024 * 1024 * 1024

    @Option(
        help: "Disk size, e.g., 20480MB or 20GB. Defaults to 50GB.",
        transform: { try parseSize($0) })
    var diskSize: UInt64 = 50 * 1024 * 1024 * 1024

    @Option(help: "Display resolution in format WIDTHxHEIGHT. Defaults to 1024x768.")
    var display: VMDisplayResolution = VMDisplayResolution(string: "1024x768")!

    @Option(
        help:
            "Path to macOS restore image (IPSW), or 'latest' to download the latest supported version. Required for macOS VMs.",
        completion: .file(extensions: ["ipsw"])
    )
    var ipsw: String?

    @Option(name: .customLong("storage"), help: "VM storage location to use or direct path to VM location")
    var storage: String?

    @Option(
        name: .customLong("unattended"),
        help: "[Preview] Preset name or path to YAML config file for unattended macOS Setup Assistant automation. Built-in presets: tahoe. Only supported for macOS VMs.",
        completion: .file(extensions: ["yml", "yaml"])
    )
    var unattended: String?

    init() {
    }

    @MainActor
    func run() async throws {
        // Validate unattended is only used with macOS
        if unattended != nil && os.lowercased() != "macos" {
            throw ValidationError("--unattended is only supported for macOS VMs")
        }

        // Load unattended config if provided
        var unattendedConfig: UnattendedConfig?
        if let unattendedArg = unattended {
            Logger.info("[Preview] Unattended setup is an experimental feature")
            let isPreset = UnattendedConfig.isPreset(name: unattendedArg)
            unattendedConfig = try UnattendedConfig.load(from: unattendedArg)
            Logger.info("Loaded unattended config", metadata: [
                "source": isPreset ? "preset:\(unattendedArg)" : unattendedArg,
                "commands": "\(unattendedConfig?.bootCommands.count ?? 0)"
            ])
        }

        let controller = LumeController()
        try await controller.create(
            name: name,
            os: os,
            diskSize: diskSize,
            cpuCount: cpu,
            memorySize: memory,
            display: display.string,
            ipsw: ipsw,
            storage: storage,
            unattendedConfig: unattendedConfig
        )
    }
}
