import ArgumentParser
import Foundation

struct Clone: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        abstract: "Clone an existing virtual machine"
    )

    @Argument(help: "Name of the source virtual machine", completion: .custom(completeVMName))
    var name: String

    @Argument(help: "Name for the cloned virtual machine")
    var newName: String

    @Option(name: .customLong("source-storage"), help: "Source VM storage location")
    var sourceStorage: String?

    @Option(name: .customLong("dest-storage"), help: "Destination VM storage location")
    var destStorage: String?

    init() {}

    @MainActor
    func run() async throws {
        // Record telemetry
        TelemetryClient.shared.record(event: TelemetryEvent.clone)

        let vmController = LumeController()
        try vmController.clone(
            name: name,
            newName: newName,
            sourceLocation: sourceStorage,
            destLocation: destStorage
        )
    }
}
