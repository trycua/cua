import Foundation

/// Protocol defining the interface for image registries (ghcr, gcs, etc.)
protocol ImageRegistry: Sendable {
    /// Pull an image from the registry
    /// - Parameters:
    ///   - image: Image reference (format: name:tag)
    ///   - name: Optional VM name (defaults to image name without tag)
    ///   - locationName: Optional storage location name or direct path
    /// - Returns: The VM directory where the image was pulled
    func pull(
        image: String,
        name: String?,
        locationName: String?
    ) async throws -> VMDirectory

    /// Push a VM to the registry
    /// - Parameters:
    ///   - vmDirPath: Path to the VM directory to push
    ///   - imageName: Name for the image in the registry
    ///   - tags: Tags to apply to the image
    ///   - chunkSizeMb: Chunk size for large files in MB
    ///   - verbose: Enable verbose logging
    ///   - dryRun: Prepare files without uploading
    ///   - reassemble: In dry-run mode, verify integrity by reassembling
    func push(
        vmDirPath: String,
        imageName: String,
        tags: [String],
        chunkSizeMb: Int,
        verbose: Bool,
        dryRun: Bool,
        reassemble: Bool
    ) async throws

    /// Get list of cached/available images
    func getImages() async throws -> [CachedImage]
}

// Default implementations for optional parameters
extension ImageRegistry {
    func pull(image: String, name: String? = nil, locationName: String? = nil) async throws -> VMDirectory {
        try await pull(image: image, name: name, locationName: locationName)
    }

    func push(
        vmDirPath: String,
        imageName: String,
        tags: [String],
        chunkSizeMb: Int = 512,
        verbose: Bool = false,
        dryRun: Bool = false,
        reassemble: Bool = false
    ) async throws {
        try await push(
            vmDirPath: vmDirPath,
            imageName: imageName,
            tags: tags,
            chunkSizeMb: chunkSizeMb,
            verbose: verbose,
            dryRun: dryRun,
            reassemble: reassemble
        )
    }
}
