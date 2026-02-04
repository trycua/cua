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
        help: "Memory size (e.g., 8, 8GB, or 8192MB). Numbers without units are treated as GB. Defaults to 8GB.",
        transform: { try parseSize($0) }
    )
    var memory: UInt64 = 8 * 1024 * 1024 * 1024

    @Option(
        help: "Disk size (e.g., 50, 50GB, or 51200MB). Numbers without units are treated as GB. Defaults to 50GB.",
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
        help: "[Preview] Preset name or path to YAML config file for unattended macOS Setup Assistant automation. Built-in presets: sequoia, tahoe. Only supported for macOS VMs.",
        completion: .file(extensions: ["yml", "yaml"])
    )
    var unattended: String?

    @Flag(
        name: .customLong("debug"),
        help: "Enable debug mode for unattended setup - saves screenshots with click coordinates")
    var debug: Bool = false

    @Option(
        name: .customLong("debug-dir"),
        help: "Custom directory for debug screenshots (defaults to unique folder in system temp)",
        completion: .directory)
    var debugDir: String?

    @Flag(
        name: .customLong("no-display"),
        help: "Do not open the VNC client during unattended setup (default: true for unattended)")
    var noDisplay: Bool = false

    @Option(
        name: .customLong("vnc-port"),
        help: "Port to use for the VNC server during unattended setup. Defaults to 0 (auto-assign)")
    var vncPort: Int = 0

    init() {
    }

    @MainActor
    func run() async throws {
        // Validate unattended is only used with macOS
        if unattended != nil && os.lowercased() != "macos" {
            throw ValidationError("--unattended is only supported for macOS VMs")
        }

        // Sanity check disk size
        let minDiskSizeGB: UInt64 = os.lowercased() == "macos" ? 30 : 10
        let minDiskSize = minDiskSizeGB * 1024 * 1024 * 1024
        if diskSize < minDiskSize {
            throw ValidationError("Disk size \(diskSize / (1024 * 1024 * 1024))GB is too small. Minimum recommended size for \(os) is \(minDiskSizeGB)GB.")
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

        // Record telemetry
        TelemetryClient.shared.record(event: TelemetryEvent.create, properties: [
            "os_type": os.lowercased(),
            "cpu": cpu,
            "memory_gb": memory / (1024 * 1024 * 1024),
            "disk_size_gb": diskSize / (1024 * 1024 * 1024),
            "has_unattended": unattendedConfig != nil
        ])

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
            unattendedConfig: unattendedConfig,
            debug: debug,
            debugDir: debugDir,
            noDisplay: unattendedConfig != nil ? true : noDisplay,  // Default to no-display for unattended
            vncPort: vncPort
        )
    }
}
