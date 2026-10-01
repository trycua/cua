import ArgumentParser
import Foundation

struct Prune: AsyncParsableCommand {
    static let configuration: CommandConfiguration = CommandConfiguration(
        commandName: "prune",
        abstract: "Remove cached images"
    )
    
    init() {
    }
    
    @MainActor
    func run() async throws {
        let manager = LumeController()
        try await manager.pruneImages()
        print("Successfully removed cached images")
    }
}