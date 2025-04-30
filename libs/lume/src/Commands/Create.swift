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

    init() {
    }

    @MainActor
    func run() async throws {
        let controller = LumeController()
        try await controller.create(
            name: name,
            os: os,
            diskSize: diskSize,
            cpuCount: cpu,
            memorySize: memory,
            display: display.string,
            ipsw: ipsw,
            storage: storage
        )
    }
}
