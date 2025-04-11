import ArgumentParser
import Foundation

struct Pull: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        abstract: "Pull a macOS image from GitHub Container Registry"
    )

    @Argument(help: "Image to pull (format: name:tag)")
    var image: String

    @Argument(
        help: "Name for the VM (defaults to image name without tag)", transform: { Optional($0) })
    var name: String?

    @Option(help: "Github Container Registry to pull from. Defaults to ghcr.io")
    var registry: String = "ghcr.io"

    @Option(help: "Organization to pull from. Defaults to trycua")
    var organization: String = "trycua"

    @Option(name: .customLong("location"), help: "VM location to use")
    var location: String?

    init() {}

    @MainActor
    func run() async throws {
        let controller = LumeController()
        try await controller.pullImage(
            image: image,
            name: name,
            registry: registry,
            organization: organization,
            locationName: location
        )
    }
}
