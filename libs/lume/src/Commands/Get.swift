import ArgumentParser
import Foundation

struct Get: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        abstract: "Get detailed information about a virtual machine"
    )

    @Argument(help: "Name of the virtual machine", completion: .custom(completeVMName))
    var name: String

    @Option(name: [.long, .customShort("f")], help: "Output format (json|text)")
    var format: FormatOption = .text

    @Option(name: .customLong("storage"), help: "VM storage location to use or direct path to VM location")
    var storage: String?

    init() {
    }

    @MainActor
    func run() async throws {
        let vmController = LumeController()
        let vm = try vmController.get(name: name, storage: storage)
        try VMDetailsPrinter.printStatus([vm.details], format: self.format)
    }
}
