import Foundation

/// Byte-level progress of an image pull.
struct PullProgress: Sendable, Equatable {
    /// Downloaded share of the image, 0 to 100.
    var percent: Double
    /// Bytes downloaded so far (finished layers, cached layers and in-flight transfers).
    var downloadedBytes: Int64
    /// Total bytes to download (0 until the manifest is known).
    var totalBytes: Int64
    /// Smoothed network transfer rate in bytes per second.
    var bytesPerSecond: Double

    static let zero = PullProgress(percent: 0, downloadedBytes: 0, totalBytes: 0, bytesPerSecond: 0)
}

typealias PullProgressHandler = @Sendable (PullProgress) -> Void

/// Protocol defining the interface for image registries (ghcr, gcs, etc.)
protocol ImageRegistry: Sendable {
    /// Pull an image from the registry
    /// - Parameters:
    ///   - image: Image reference (format: name:tag)
    ///   - name: Optional VM name (defaults to image name without tag)
    ///   - locationName: Optional storage location name or direct path
    ///   - force: Force re-download even if image exists locally
    /// - Returns: The VM directory where the image was pulled
    func pull(
        image: String,
        name: String?,
        locationName: String?,
        force: Bool,
        progressHandler: PullProgressHandler?
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
    ///   - legacy: Use legacy Lume LZ4-chunked format instead of OCI-compliant format
    func push(
        vmDirPath: String,
        imageName: String,
        tags: [String],
        chunkSizeMb: Int,
        verbose: Bool,
        dryRun: Bool,
        reassemble: Bool,
        singleLayer: Bool,
        legacy: Bool
    ) async throws

    /// Get list of cached/available images
    func getImages() async throws -> [CachedImage]
}

// Default implementations for optional parameters
extension ImageRegistry {
    func pull(image: String, name: String? = nil, locationName: String? = nil, force: Bool = false, progressHandler: PullProgressHandler? = nil) async throws -> VMDirectory {
        try await pull(image: image, name: name, locationName: locationName, force: force, progressHandler: progressHandler)
    }

    func push(
        vmDirPath: String,
        imageName: String,
        tags: [String],
        chunkSizeMb: Int = 512,
        verbose: Bool = false,
        dryRun: Bool = false,
        reassemble: Bool = false,
        singleLayer: Bool = false,
        legacy: Bool = false
    ) async throws {
        try await push(
            vmDirPath: vmDirPath,
            imageName: imageName,
            tags: tags,
            chunkSizeMb: chunkSizeMb,
            verbose: verbose,
            dryRun: dryRun,
            reassemble: reassemble,
            singleLayer: singleLayer,
            legacy: legacy
        )
    }
}
