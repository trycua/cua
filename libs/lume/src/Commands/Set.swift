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

    @Option(help: "New memory size (e.g., 8, 8GB, or 8192MB). Numbers without units are treated as GB.", transform: { try parseSize($0) })
    var memory: UInt64?

    @Option(help: "New disk size (e.g., 50, 50GB, or 51200MB). Increase only. For macOS VMs this relocates the recovery partition and expands the main APFS container; the VM must be stopped; may take several minutes. Numbers without units are treated as GB.", transform: { try parseSize($0) })
    var diskSize: UInt64?

    @Option(help: "New display resolution in format WIDTHxHEIGHT.")
    var display: VMDisplayResolution?

    @Option(name: .customLong("storage"), help: "VM storage location to use or direct path to VM location")
    var storage: String?

    @Flag(name: .customLong("no-backup"), help: "Skip the pre-resize disk backup (macOS resize only). Faster, but a failure cannot be rolled back automatically.")
    var noBackup: Bool = false

    @Flag(name: .customLong("keep-backup"), help: "Keep the pre-resize disk backup after a successful macOS resize instead of deleting it.")
    var keepBackup: Bool = false

    @Flag(name: .customLong("dry-run"), help: "Validate the disk-resize plan and print it without modifying anything (macOS resize only).")
    var dryRun: Bool = false

    init() {
    }

    @MainActor
    func run() async throws {
        if (noBackup || keepBackup || dryRun) && diskSize == nil {
            throw ValidationError("--no-backup, --keep-backup and --dry-run are only valid together with --disk-size")
        }
        if noBackup && keepBackup {
            throw ValidationError("--no-backup and --keep-backup cannot be used together")
        }
        let vmController = LumeController()
        try vmController.updateSettings(
            name: name,
            cpu: cpu,
            memory: memory,
            diskSize: diskSize,
            display: display?.string,
            storage: storage,
            noBackup: noBackup,
            keepBackup: keepBackup,
            dryRun: dryRun
        )
    }
}
