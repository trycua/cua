import ArgumentParser
import Foundation

struct IPSW: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        abstract: "Get macOS restore image IPSW URL",
        discussion: "Download IPSW file manually, then use in create command with --ipsw"
    )
    
    init() {
        
    }
    
    @MainActor
    func run() async throws {
        let vmController = LumeController()
        let url = try await vmController.getLatestIPSWURL()
        print(url.absoluteString)
    }
}