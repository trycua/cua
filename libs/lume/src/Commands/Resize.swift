import ArgumentParser
import Foundation

struct Resize: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        abstract: "Resize a virtual machine's disk image"
    )

    @Argument(help: "Name of the virtual machine to resize", completion: .custom(completeVMName))
    var name: String

    @Flag(help: "Compact the disk image by removing unused space (zeroed blocks)")
    var compact: Bool = false

    @Option(help: "Set absolute disk size (e.g., 50GB, 100GB)")
    var size: String?

    @Option(help: "Expand the disk size by the specified amount (e.g., 10GB, 5GB)")
    var expandBy: String?

    @Option(name: .customLong("storage"), help: "VM storage location to use or direct path to VM location")
    var storage: String?

    @Flag(name: .long, help: "Force resize without confirmation")
    var force = false

    init() {}

    @MainActor
    func run() async throws {
        // Validate that at least one resize option is provided
        guard compact || size != nil || expandBy != nil else {
            throw ValidationError("Must specify at least one resize option: --compact, --size, or --expand-by")
        }

        // Validate that only one resize method is used at a time
        let optionsCount = [compact, size != nil, expandBy != nil].filter { $0 }.count
        guard optionsCount == 1 else {
            throw ValidationError("Cannot use multiple resize options simultaneously. Choose one: --compact, --size, or --expand-by")
        }

        // Record telemetry
        TelemetryClient.shared.record(event: TelemetryEvent.resize)

        let vmController = LumeController()
        try await vmController.resize(
            name: name,
            compact: compact,
            size: size,
            expandBy: expandBy,
            storage: storage,
            force: force
        )
    }
}
