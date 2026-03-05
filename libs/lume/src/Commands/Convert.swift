import ArgumentParser
import Foundation

struct Convert: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        abstract: "Convert a legacy Lume image to OCI-compliant format",
        discussion: """
            Pulls a legacy Lume image from the registry, re-pushes it in OCI-compliant format
            under a new name/tag, then removes the temporary local VM.

            Example:
              lume convert macos-sequoia:latest trycua/macos-sequoia:latest-oci
        """
    )

    @Argument(help: "Source image to convert (legacy format, e.g. macos-sequoia:latest)")
    var sourceImage: String

    @Argument(help: "Target image to push in OCI format (format: name:tag)")
    var targetImage: String

    @Option(parsing: .upToNextOption, help: "Additional tags to push the OCI image to")
    var additionalTags: [String] = []

    @Option(help: "Registry to pull from and push to. Defaults to ghcr.io")
    var registry: String = "ghcr.io"

    @Option(help: "Organization. Defaults to trycua")
    var organization: String = "trycua"

    @Flag(name: .long, help: "Enable verbose logging")
    var verbose: Bool = false

    @Flag(name: .long, help: "Prepare files without uploading to registry")
    var dryRun: Bool = false

    init() {}

    @MainActor
    func run() async throws {
        TelemetryClient.shared.record(event: TelemetryEvent.push)

        let targetComponents = targetImage.split(separator: ":")
        guard targetComponents.count == 2, let primaryTag = targetComponents.last else {
            throw ValidationError("Invalid target image format. Expected format: name:tag")
        }
        let targetName = String(targetComponents.first!)

        var allTags: Swift.Set<String> = [String(primaryTag)]
        allTags.formUnion(additionalTags)

        // Use a unique temp VM name to avoid collisions
        let tempVMName = "__lume_convert_\(UUID().uuidString.prefix(8))"

        let controller = LumeController()

        Logger.info(
            "Converting legacy image to OCI-compliant format",
            metadata: [
                "source": sourceImage,
                "target": targetImage,
                "registry": registry,
                "organization": organization,
            ])

        // Step 1: Pull the legacy source image into a temp VM
        Logger.info("Step 1/3: Pulling source image '\(sourceImage)'…")
        do {
            try await controller.pullImage(
                image: sourceImage,
                name: tempVMName,
                registry: registry,
                organization: organization
            )
        } catch {
            Logger.error("Pull failed, cleaning up temp VM: \(error.localizedDescription)")
            try? await controller.delete(name: tempVMName)
            throw error
        }

        // Step 2: Push the temp VM in OCI-compliant format
        Logger.info("Step 2/3: Pushing '\(sourceImage)' as OCI-compliant '\(targetImage)'…")
        do {
            try await controller.pushImage(
                name: tempVMName,
                imageName: targetName,
                tags: Array(allTags),
                registry: registry,
                organization: organization,
                verbose: verbose,
                dryRun: dryRun,
                reassemble: false,
                legacy: false  // OCI-compliant output
            )
        } catch {
            // Always clean up the temp VM even if push fails
            Logger.error("Push failed, cleaning up temp VM: \(error.localizedDescription)")
            try? await controller.delete(name: tempVMName)
            throw error
        }

        // Step 3: Remove the temp VM
        Logger.info("Step 3/3: Cleaning up temporary VM…")
        try await controller.delete(name: tempVMName)

        Logger.info(
            "Conversion complete",
            metadata: [
                "source": sourceImage,
                "target": targetImage,
            ])
    }
}
