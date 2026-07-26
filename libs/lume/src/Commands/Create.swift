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
        help: "Disk size (e.g., 100, 100GB, or 102400MB). Numbers without units are treated as GB. Defaults to 100GB for macOS and 50GB for Linux.",
        transform: { try parseSize($0) })
    var diskSize: UInt64?

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
        help: "Prepare macOS unattended setup offline after install. Preset name or YAML path is accepted for compatibility. Built-in presets: sequoia, tahoe. Only supported for macOS VMs.",
        completion: .file(extensions: ["yml", "yaml"])
    )
    var unattended: String?

    @Flag(
        name: .customLong("debug"),
        help: "Compatibility flag; ignored by offline setup.")
    var debug: Bool = false

    @Option(
        name: .customLong("debug-dir"),
        help: "Compatibility option; ignored by offline setup.",
        completion: .directory)
    var debugDir: String?

    @Flag(
        name: .customLong("no-display"),
        help: "Compatibility flag; offline setup verifies headlessly.")
    var noDisplay: Bool = false

    @Option(
        name: .customLong("vnc-port"),
        help: "Port to use for the temporary verification VNC server. Defaults to 0 (auto-assign).")
    var vncPort: Int = 0

    @Option(
        name: .customLong("network"),
        help: "Network mode: 'nat' (default), 'bridged' (auto-select interface), or 'bridged:<interface>' (e.g. 'bridged:en0')")
    var network: String = "nat"

    private var parsedNetworkMode: NetworkMode {
        get throws {
            guard let mode = NetworkMode.parse(network) else {
                throw ValidationError(
                    "Invalid network mode '\(network)'. Expected 'nat', 'bridged', or 'bridged:<interface>'."
                )
            }
            return mode
        }
    }

    init() {
    }

    @MainActor
    func run() async throws {
        let diskSize = diskSize ?? Self.defaultDiskSize(for: os)

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
            let isPreset = UnattendedConfig.isPreset(name: unattendedArg)
            unattendedConfig = try UnattendedConfig.load(from: unattendedArg)
            Logger.info("Loaded unattended config", metadata: [
                "source": isPreset ? "preset:\(unattendedArg)" : unattendedArg,
                "config_boot_commands": "\(unattendedConfig?.bootCommands.count ?? 0)"
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
            vncPort: vncPort,
            networkMode: parsedNetworkMode
        )
    }

    static func defaultDiskSize(for os: String) -> UInt64 {
        let sizeInGB: UInt64 = os.lowercased() == "macos" ? 100 : 50
        return sizeInGB * 1024 * 1024 * 1024
    }
}
