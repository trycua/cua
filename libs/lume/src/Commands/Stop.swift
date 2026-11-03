import ArgumentParser
import Foundation

struct Stop: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        abstract: "Stop a virtual machine"
    )

    @Argument(help: "Name of the virtual machine", completion: .custom(completeVMName))
    var name: String

    @Option(name: .customLong("storage"), help: "VM storage location to use or direct path to VM location")
    var storage: String?

    @Flag(
        name: .long,
        help: "Power off the VM immediately instead of attempting a graceful shutdown first")
    var force = false

    @Option(
        name: .long,
        help: "Seconds to wait for a graceful shutdown before forcing power off")
    var timeout: Int = 10

    init() {
    }

    func validate() throws {
        if timeout < 0 {
            throw ValidationError("--timeout must be zero or a positive number of seconds.")
        }
    }

    @MainActor
    func run() async throws {
        // Record telemetry
        TelemetryClient.shared.record(event: TelemetryEvent.stop)

        let vmController = LumeController()
        try await vmController.stopVM(
            name: name, storage: storage, force: force, timeout: TimeInterval(timeout))
    }
}
