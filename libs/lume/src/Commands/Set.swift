import ArgumentParser
import Foundation

struct Set: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        abstract: "Set new values for CPU, memory, and disk size of a virtual machine"
    )

    @Argument(help: "Name of the virtual machine", completion: .custom(completeVMName))
    var name: String

    @Option(help: "New number of CPU cores")
    var cpu: Int?

    @Option(help: "New memory size, e.g., 8192MB or 8GB.", transform: { try parseSize($0) })
    var memory: UInt64?

    @Option(help: "New disk size, e.g., 20480MB or 20GB.", transform: { try parseSize($0) })
    var diskSize: UInt64?

    @Option(help: "New display resolution in format WIDTHxHEIGHT.")
    var display: VMDisplayResolution?

    @Option(name: .customLong("storage"), help: "VM storage location to use or direct path to VM location")
    var storage: String?

    init() {
    }

    @MainActor
    func run() async throws {
        let vmController = LumeController()
        try vmController.updateSettings(
            name: name,
            cpu: cpu,
            memory: memory,
            diskSize: diskSize,
            display: display?.string,
            storage: storage
        )
    }
}
