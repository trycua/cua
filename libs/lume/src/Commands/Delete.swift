import ArgumentParser
import Foundation

struct Delete: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        abstract: "Delete a virtual machine"
    )

    @Argument(help: "Name of the virtual machine to delete", completion: .custom(completeVMName))
    var name: String

    @Flag(name: .long, help: "Force deletion without confirmation")
    var force = false

    @Option(name: .customLong("storage"), help: "VM storage location to use or direct path to VM location")
    var storage: String?

    init() {}

    @MainActor
    func run() async throws {
        if !force {
            print(
                "Are you sure you want to delete the virtual machine '\(name)'? [y/N] ",
                terminator: "")
            guard let response = readLine()?.lowercased(),
                response == "y" || response == "yes"
            else {
                print("Deletion cancelled")
                return
            }
        }

        let vmController = LumeController()
        try await vmController.delete(name: name, storage: storage)
    }
}
