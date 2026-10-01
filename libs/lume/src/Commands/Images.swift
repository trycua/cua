import ArgumentParser
import Foundation

struct Images: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        abstract: "List available macOS images from local cache"
    )
    
    @Option(help: "Organization to list from. Defaults to trycua")
    var organization: String = "trycua"
    
    init() {}
    
    @MainActor
    func run() async throws {
        let vmController = LumeController()
        _ = try await vmController.getImages(organization: organization)
    }
}
