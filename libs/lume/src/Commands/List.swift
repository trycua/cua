import ArgumentParser
import Foundation

struct List: AsyncParsableCommand {
    static let configuration: CommandConfiguration = CommandConfiguration(
        commandName: "ls",
        abstract: "List virtual machines"
    )
    
    @Option(name: [.long, .customShort("f")], help: "Output format (json|text)")
    var format: FormatOption = .text
    
    @Option(name: .long, help: "Filter by storage location name")
    var storage: String?

    init() {
    }
    
    @MainActor
    func run() async throws {
        let manager = LumeController()
        let vms = try manager.list(storage: self.storage)
        if vms.isEmpty && self.format == .text {
            if let storageName = self.storage {
                print("No virtual machines found in storage '\(storageName)'")
            } else {
                print("No virtual machines found")
            }
        } else {
            try VMDetailsPrinter.printStatus(vms, format: self.format)
        }
    }
}
