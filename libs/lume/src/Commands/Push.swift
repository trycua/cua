import ArgumentParser
import Foundation

struct Push: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        abstract: "Push a macOS VM to GitHub Container Registry"
    )

    @Argument(help: "Name of the VM to push")
    var name: String

    @Argument(help: "Image tag to push (format: name:tag)")
    var image: String

    @Option(parsing: .upToNextOption, help: "Additional tags to push the same image to")
    var additionalTags: [String] = []

    @Option(help: "Github Container Registry to push to. Defaults to ghcr.io")
    var registry: String = "ghcr.io"

    @Option(help: "Organization to push to. Defaults to trycua")
    var organization: String = "trycua"

    @Option(name: .customLong("storage"), help: "VM storage location to use")
    var storage: String?

    @Option(help: "Chunk size for large files in MB. Defaults to 512.")
    var chunkSizeMb: Int = 512

    @Flag(name: .long, help: "Enable verbose logging")
    var verbose: Bool = false

    @Flag(name: .long, help: "Prepare files without uploading to registry")
    var dryRun: Bool = false
    
    @Flag(name: .long, help: "In dry-run mode, also reassemble chunks to verify integrity")
    var reassemble: Bool = true

    init() {}

    @MainActor
    func run() async throws {
        // Record telemetry
        TelemetryClient.shared.record(event: TelemetryEvent.push)

        let controller = LumeController()

        // Parse primary image name and tag
        let components = image.split(separator: ":")
        guard components.count == 2, let primaryTag = components.last else {
            throw ValidationError("Invalid primary image format. Expected format: name:tag")
        }
        let imageName = String(components.first!)
        
        // Combine primary and additional tags, ensuring uniqueness
        var allTags: Swift.Set<String> = []
        allTags.insert(String(primaryTag))
        allTags.formUnion(additionalTags)
        
        guard !allTags.isEmpty else {
             throw ValidationError("At least one tag must be provided.")
        }
        
        try await controller.pushImage(
            name: name,
            imageName: imageName, // Pass base image name
            tags: Array(allTags), // Pass array of all unique tags
            registry: registry,
            organization: organization,
            storage: storage,
            chunkSizeMb: chunkSizeMb,
            verbose: verbose,
            dryRun: dryRun,
            reassemble: reassemble
        )
    }
} 