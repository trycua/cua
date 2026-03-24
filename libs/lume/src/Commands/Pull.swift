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

    @Option(name: .customLong("storage"), help: "VM storage location to use or direct path to VM location")
    var storage: String?

    @Option(name: .long, help: "Registry username for authentication")
    var username: String?

    @Option(name: .long, help: "Registry password for authentication")
    var password: String?

    @Flag(name: .long, help: "Force re-download even if image exists")
    var force: Bool = false

    @Flag(name: .long, help: "Enable verbose logging")
    var verbose: Bool = false

    init() {}

    @MainActor
    func run() async throws {
        if verbose { Logger.setVerbose() }

        // Record telemetry - only capture image name without tag for privacy
        let imageName = image.split(separator: ":").first.map(String.init) ?? image
        TelemetryClient.shared.record(event: TelemetryEvent.pull, properties: [
            "image_name": imageName
        ])

        let controller = LumeController()
        try await controller.pullImage(
            image: image,
            name: name,
            registry: registry,
            organization: organization,
            storage: storage,
            username: username,
            password: password,
            force: force
        )
    }
}
