import ArgumentParser
import Darwin
import Foundation
import Swift

struct Layer: Codable, Equatable {
    let mediaType: String
    let digest: String
    let size: Int
}

struct Manifest: Codable {
    let layers: [Layer]
    let config: Layer?
    let mediaType: String
    let schemaVersion: Int
}

struct RepositoryTag: Codable {
    let name: String
    let tags: [String]
}

struct RepositoryList: Codable {
    let repositories: [String]
}

struct RepositoryTags: Codable {
    let name: String
    let tags: [String]
}

struct CachedImage {
    let repository: String
    let imageId: String
    let manifestId: String
}

struct ImageMetadata: Codable {
    let image: String
    let manifestId: String
    let timestamp: Date
}

actor ProgressTracker {
    private var totalBytes: Int64 = 0
    private var downloadedBytes: Int64 = 0
    private var progressLogger = ProgressLogger(threshold: 0.01)
    private var totalFiles: Int = 0
    private var completedFiles: Int = 0

    func setTotal(_ total: Int64, files: Int) {
        totalBytes = total
        totalFiles = files
    }

    func addProgress(_ bytes: Int64) {
        downloadedBytes += bytes
        let progress = Double(downloadedBytes) / Double(totalBytes)
        progressLogger.logProgress(current: progress, context: "Downloading Image")
    }
}

actor TaskCounter {
    private var count: Int = 0

    func increment() { count += 1 }
    func decrement() { count -= 1 }
    func current() -> Int { count }
}

class ImageContainerRegistry: @unchecked Sendable {
    private let registry: String
    private let organization: String
    private let progress = ProgressTracker()
    private let cacheDirectory: URL
    private let downloadLock = NSLock()
    private var activeDownloads: [String] = []

    init(registry: String, organization: String) {
        self.registry = registry
        self.organization = organization

        // Get cache directory from settings
        let cacheDir = SettingsManager.shared.getCacheDirectory()
        let expandedCacheDir = (cacheDir as NSString).expandingTildeInPath
        self.cacheDirectory = URL(fileURLWithPath: expandedCacheDir)
            .appendingPathComponent("ghcr")

        try? FileManager.default.createDirectory(
            at: cacheDirectory, withIntermediateDirectories: true)

        // Create organization directory
        let orgDir = cacheDirectory.appendingPathComponent(organization)
        try? FileManager.default.createDirectory(at: orgDir, withIntermediateDirectories: true)
    }

    private func getManifestIdentifier(_ manifest: Manifest, manifestDigest: String) -> String {
        // Use the manifest's own digest as the identifier
        return manifestDigest.replacingOccurrences(of: ":", with: "_")
    }

    private func getShortImageId(_ digest: String) -> String {
        // Take first 12 characters of the digest after removing the "sha256:" prefix
        let id = digest.replacingOccurrences(of: "sha256:", with: "")
        return String(id.prefix(12))
    }

    private func getImageCacheDirectory(manifestId: String) -> URL {
        return
            cacheDirectory
            .appendingPathComponent(organization)
            .appendingPathComponent(manifestId)
    }

    private func getCachedManifestPath(manifestId: String) -> URL {
        return getImageCacheDirectory(manifestId: manifestId).appendingPathComponent(
            "manifest.json")
    }

    private func getCachedLayerPath(manifestId: String, digest: String) -> URL {
        return getImageCacheDirectory(manifestId: manifestId).appendingPathComponent(
            digest.replacingOccurrences(of: ":", with: "_"))
    }

    private func setupImageCache(manifestId: String) throws {
        let cacheDir = getImageCacheDirectory(manifestId: manifestId)
        // Remove existing cache if it exists
        if FileManager.default.fileExists(atPath: cacheDir.path) {
            try FileManager.default.removeItem(at: cacheDir)
            // Ensure it's completely removed
            while FileManager.default.fileExists(atPath: cacheDir.path) {
                try? FileManager.default.removeItem(at: cacheDir)
            }
        }
        try FileManager.default.createDirectory(at: cacheDir, withIntermediateDirectories: true)
    }

    private func loadCachedManifest(manifestId: String) -> Manifest? {
        let manifestPath = getCachedManifestPath(manifestId: manifestId)
        guard let data = try? Data(contentsOf: manifestPath) else { return nil }
        return try? JSONDecoder().decode(Manifest.self, from: data)
    }

    private func validateCache(manifest: Manifest, manifestId: String) -> Bool {
        // First check if manifest exists and matches
        guard let cachedManifest = loadCachedManifest(manifestId: manifestId),
            cachedManifest.layers == manifest.layers
        else {
            return false
        }

        // Then verify all layer files exist
        for layer in manifest.layers {
            let cachedLayer = getCachedLayerPath(manifestId: manifestId, digest: layer.digest)
            if !FileManager.default.fileExists(atPath: cachedLayer.path) {
                return false
            }
        }

        return true
    }

    private func saveManifest(_ manifest: Manifest, manifestId: String) throws {
        let manifestPath = getCachedManifestPath(manifestId: manifestId)
        try JSONEncoder().encode(manifest).write(to: manifestPath)
    }

    private func isDownloading(_ digest: String) -> Bool {
        downloadLock.lock()
        defer { downloadLock.unlock() }
        return activeDownloads.contains(digest)
    }

    private func markDownloadStarted(_ digest: String) {
        downloadLock.lock()
        if !activeDownloads.contains(digest) {
            activeDownloads.append(digest)
        }
        downloadLock.unlock()
    }

    private func markDownloadComplete(_ digest: String) {
        downloadLock.lock()
        activeDownloads.removeAll { $0 == digest }
        downloadLock.unlock()
    }

    private func waitForExistingDownload(_ digest: String, cachedLayer: URL) async throws {
        while isDownloading(digest) {
            try await Task.sleep(nanoseconds: 1_000_000_000)  // Sleep for 1 second
            if FileManager.default.fileExists(atPath: cachedLayer.path) {
                return  // File is now available
            }
        }
    }

    private func saveImageMetadata(image: String, manifestId: String) throws {
        let metadataPath = getImageCacheDirectory(manifestId: manifestId).appendingPathComponent(
            "metadata.json")
        let metadata = ImageMetadata(
            image: image,
            manifestId: manifestId,
            timestamp: Date()
        )
        try JSONEncoder().encode(metadata).write(to: metadataPath)
    }

    private func cleanupOldVersions(currentManifestId: String, image: String) throws {
        Logger.info(
            "Checking for old versions of image to clean up",
            metadata: [
                "image": image,
                "current_manifest_id": currentManifestId,
            ])

        let orgDir = cacheDirectory.appendingPathComponent(organization)
        guard FileManager.default.fileExists(atPath: orgDir.path) else { return }

        let contents = try FileManager.default.contentsOfDirectory(atPath: orgDir.path)
        for item in contents {
            if item == currentManifestId { continue }

            let itemPath = orgDir.appendingPathComponent(item)
            let metadataPath = itemPath.appendingPathComponent("metadata.json")

            if let metadataData = try? Data(contentsOf: metadataPath),
                let metadata = try? JSONDecoder().decode(ImageMetadata.self, from: metadataData)
            {
                if metadata.image == image {
                    try FileManager.default.removeItem(at: itemPath)
                    Logger.info(
                        "Removed old version of image",
                        metadata: [
                            "image": image,
                            "old_manifest_id": item,
                        ])
                }
                continue
            }

            Logger.info(
                "Skipping cleanup check for item without metadata", metadata: ["item": item])
        }
    }

    public func pull(
        image: String,
        name: String?,
        locationName: String? = nil
    ) async throws {
        guard !image.isEmpty else {
            throw ValidationError("Image name cannot be empty")
        }

        let home = Home()

        // Use provided name or derive from image
        let vmName = name ?? image.split(separator: ":").first.map(String.init) ?? ""
        let vmDir = try home.getVMDirectory(vmName, locationName: locationName)

        // Parse image name and tag
        let components = image.split(separator: ":")
        guard components.count == 2, let tag = components.last else {
            throw ValidationError("Invalid image format. Expected format: name:tag")
        }

        let imageName = String(components.first!)
        let imageTag = String(tag)

        Logger.info(
            "Pulling image",
            metadata: [
                "image": image,
                "name": vmName,
                "location": locationName ?? "default",
                "registry": registry,
                "organization": organization,
            ])

        // Get anonymous token
        Logger.info("Getting registry authentication token")
        let token = try await getToken(repository: "\(self.organization)/\(imageName)")

        // Fetch manifest
        Logger.info("Fetching Image manifest")
        let (manifest, manifestDigest): (Manifest, String) = try await fetchManifest(
            repository: "\(self.organization)/\(imageName)",
            tag: imageTag,
            token: token
        )

        // Get manifest identifier using the manifest's own digest
        let manifestId = getManifestIdentifier(manifest, manifestDigest: manifestDigest)

        Logger.info(
            "Pulling image",
            metadata: [
                "repository": imageName,
                "manifest_id": manifestId,
            ])

        // Create temporary directory for the entire VM setup
        let tempVMDir = FileManager.default.temporaryDirectory.appendingPathComponent(
            "lume_vm_\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: tempVMDir, withIntermediateDirectories: true)
        defer {
            try? FileManager.default.removeItem(at: tempVMDir)
        }

        // Check if we have a valid cached version and noCache is false
        Logger.info("Checking cache for manifest ID: \(manifestId)")
        if validateCache(manifest: manifest, manifestId: manifestId) {
            Logger.info("Using cached version of image")
            try await copyFromCache(manifest: manifest, manifestId: manifestId, to: tempVMDir)
        } else {
            // Clean up old versions of this repository before setting up new cache
            try cleanupOldVersions(currentManifestId: manifestId, image: imageName)

            Logger.info("Cache miss or invalid cache, setting up new cache")
            // Setup new cache directory
            try setupImageCache(manifestId: manifestId)
            // Save new manifest
            try saveManifest(manifest, manifestId: manifestId)

            // Save image metadata
            try saveImageMetadata(
                image: imageName,
                manifestId: manifestId
            )

            // Create temporary directory for new downloads
            let tempDownloadDir = FileManager.default.temporaryDirectory.appendingPathComponent(
                UUID().uuidString)
            try FileManager.default.createDirectory(
                at: tempDownloadDir, withIntermediateDirectories: true)
            defer {
                try? FileManager.default.removeItem(at: tempDownloadDir)
            }

            // Set total size and file count
            let totalFiles = manifest.layers.filter {
                $0.mediaType != "application/vnd.oci.empty.v1+json"
            }.count
            let totalSize = manifest.layers.reduce(0) { $0 + Int64($1.size) }
            Logger.info(
                "Total download size: \(ByteCountFormatter.string(fromByteCount: totalSize, countStyle: .file))"
            )
            await progress.setTotal(totalSize, files: totalFiles)

            // Process layers with limited concurrency
            Logger.info("Processing Image layers")
            Logger.info(
                "This may take several minutes depending on the image size and your internet connection. Please wait..."
            )
            var diskParts: [(Int, URL)] = []
            var totalParts = 0
            let maxConcurrentTasks = 5
            let counter = TaskCounter()

            // Use a more efficient approach for memory-constrained systems
            let memoryConstrained = determineIfMemoryConstrained()
            Logger.info(
                memoryConstrained
                    ? "Using memory-optimized mode for disk parts"
                    : "Using standard mode for disk parts")

            try await withThrowingTaskGroup(of: Int64.self) { group in
                for layer in manifest.layers {
                    if layer.mediaType == "application/vnd.oci.empty.v1+json" {
                        continue
                    }

                    while await counter.current() >= maxConcurrentTasks {
                        _ = try await group.next()
                        await counter.decrement()
                    }

                    if let partInfo = extractPartInfo(from: layer.mediaType) {
                        let (partNum, total) = partInfo
                        totalParts = total

                        let cachedLayer = getCachedLayerPath(
                            manifestId: manifestId, digest: layer.digest)
                        let digest = layer.digest
                        let size = layer.size

                        // For memory-optimized mode - point directly to cache when possible
                        if memoryConstrained
                            && FileManager.default.fileExists(atPath: cachedLayer.path)
                        {
                            // Use the cached file directly
                            diskParts.append((partNum, cachedLayer))

                            // Still need to account for progress
                            group.addTask { @Sendable [self] in
                                await counter.increment()
                                await progress.addProgress(Int64(size))
                                await counter.decrement()
                                return Int64(size)
                            }
                            continue
                        } else {
                            let partURL = tempDownloadDir.appendingPathComponent(
                                "disk.img.part.\(partNum)")
                            diskParts.append((partNum, partURL))

                            group.addTask { @Sendable [self] in
                                await counter.increment()

                                if FileManager.default.fileExists(atPath: cachedLayer.path) {
                                    try FileManager.default.copyItem(at: cachedLayer, to: partURL)
                                    await progress.addProgress(Int64(size))
                                } else {
                                    // Check if this layer is already being downloaded and we're not skipping cache
                                    if isDownloading(digest) {
                                        try await waitForExistingDownload(
                                            digest, cachedLayer: cachedLayer)
                                        if FileManager.default.fileExists(atPath: cachedLayer.path)
                                        {
                                            try FileManager.default.copyItem(
                                                at: cachedLayer, to: partURL)
                                            await progress.addProgress(Int64(size))
                                            return Int64(size)
                                        }
                                    }

                                    // Start new download
                                    markDownloadStarted(digest)

                                    try await self.downloadLayer(
                                        repository: "\(self.organization)/\(imageName)",
                                        digest: digest,
                                        mediaType: layer.mediaType,
                                        token: token,
                                        to: partURL,
                                        maxRetries: 5,
                                        progress: progress
                                    )

                                    // Cache the downloaded layer if not in noCache mode
                                    if FileManager.default.fileExists(atPath: cachedLayer.path) {
                                        try FileManager.default.removeItem(at: cachedLayer)
                                    }
                                    try FileManager.default.copyItem(
                                        at: partURL, to: cachedLayer)
                                    markDownloadComplete(digest)
                                }

                                await counter.decrement()
                                return Int64(size)
                            }
                            continue
                        }
                    } else {
                        let mediaType = layer.mediaType
                        let digest = layer.digest
                        let size = layer.size

                        let outputURL: URL
                        switch mediaType {
                        case "application/vnd.oci.image.layer.v1.tar":
                            outputURL = tempDownloadDir.appendingPathComponent("disk.img")
                        case "application/vnd.oci.image.config.v1+json":
                            outputURL = tempDownloadDir.appendingPathComponent("config.json")
                        case "application/octet-stream":
                            outputURL = tempDownloadDir.appendingPathComponent("nvram.bin")
                        default:
                            continue
                        }

                        group.addTask { @Sendable [self] in
                            await counter.increment()

                            let cachedLayer = getCachedLayerPath(
                                manifestId: manifestId, digest: digest)

                            if FileManager.default.fileExists(atPath: cachedLayer.path) {
                                try FileManager.default.copyItem(at: cachedLayer, to: outputURL)
                                await progress.addProgress(Int64(size))
                            } else {
                                // Check if this layer is already being downloaded and we're not skipping cache
                                if isDownloading(digest) {
                                    try await waitForExistingDownload(
                                        digest, cachedLayer: cachedLayer)
                                    if FileManager.default.fileExists(atPath: cachedLayer.path) {
                                        try FileManager.default.copyItem(
                                            at: cachedLayer, to: outputURL)
                                        await progress.addProgress(Int64(size))
                                        return Int64(size)
                                    }
                                }

                                // Start new download
                                markDownloadStarted(digest)

                                try await self.downloadLayer(
                                    repository: "\(self.organization)/\(imageName)",
                                    digest: digest,
                                    mediaType: mediaType,
                                    token: token,
                                    to: outputURL,
                                    maxRetries: 5,
                                    progress: progress
                                )

                                // Cache the downloaded layer if not in noCache mode
                                if FileManager.default.fileExists(atPath: cachedLayer.path) {
                                    try FileManager.default.removeItem(at: cachedLayer)
                                }
                                try FileManager.default.copyItem(at: outputURL, to: cachedLayer)
                                markDownloadComplete(digest)
                            }

                            await counter.decrement()
                            return Int64(size)
                        }
                    }
                }

                // Wait for remaining tasks
                for try await _ in group {}
            }
            Logger.info("")  // New line after progress

            // Handle disk parts if present
            if !diskParts.isEmpty {
                Logger.info("Reassembling disk image...")
                let outputURL = tempVMDir.appendingPathComponent("disk.img")
                try FileManager.default.createDirectory(
                    at: outputURL.deletingLastPathComponent(), withIntermediateDirectories: true)

                // Create empty output file
                FileManager.default.createFile(atPath: outputURL.path, contents: nil)
                let outputHandle = try FileHandle(forWritingTo: outputURL)
                defer { try? outputHandle.close() }

                var totalWritten: UInt64 = 0
                let expectedTotalSize = UInt64(
                    manifest.layers.filter { extractPartInfo(from: $0.mediaType) != nil }.reduce(0)
                    { $0 + $1.size })

                // Process parts in order
                for partNum in 1...totalParts {
                    guard let (_, partURL) = diskParts.first(where: { $0.0 == partNum }) else {
                        throw PullError.missingPart(partNum)
                    }

                    let inputHandle = try FileHandle(forReadingFrom: partURL)
                    defer {
                        try? inputHandle.close()
                        // Don't delete the part file if we're in cache mode and the part is from cache
                        if !partURL.path.contains(cacheDirectory.path) {
                            try? FileManager.default.removeItem(at: partURL)
                        }
                    }

                    // On low memory systems, be more aggressive with releasing memory
                    let memoryConstrained = determineIfMemoryConstrained()
                    var chunksProcessed = 0

                    while let data = try inputHandle.read(upToCount: getOptimalChunkSize()) {
                        try autoreleasepool {
                            try outputHandle.write(contentsOf: data)
                            totalWritten += UInt64(data.count)

                            // Only log progress every 5% to reduce log noise
                            let progress: Double =
                                Double(totalWritten) / Double(expectedTotalSize) * 100
                            let roundedProgress = Int(progress / 5) * 5
                            if roundedProgress != Int(
                                (Double(totalWritten - UInt64(data.count))
                                    / Double(expectedTotalSize) * 100)
                                    / 5) * 5
                            {
                                Logger.info("Reassembling disk image: \(roundedProgress)%")
                            }

                            // Force more frequent autoreleases on memory-constrained systems
                            chunksProcessed += 1
                            if memoryConstrained && chunksProcessed % 10 == 0 {
                                try outputHandle.synchronize()
                            }
                        }
                    }

                    // Make sure we explicitly close handles after each part to free resources
                    try? inputHandle.synchronize()
                    try inputHandle.close()
                }

                // Verify final size
                let finalSize =
                    try FileManager.default.attributesOfItem(atPath: outputURL.path)[.size]
                    as? UInt64 ?? 0
                Logger.info(
                    "Final disk image size: \(ByteCountFormatter.string(fromByteCount: Int64(finalSize), countStyle: .file))"
                )
                Logger.info(
                    "Expected size: \(ByteCountFormatter.string(fromByteCount: Int64(expectedTotalSize), countStyle: .file))"
                )

                if finalSize != expectedTotalSize {
                    Logger.info(
                        "Warning: Final size (\(finalSize) bytes) differs from expected size (\(expectedTotalSize) bytes)"
                    )
                }

                Logger.info("Disk image reassembled successfully")
            } else {
                // Copy single disk image if it exists
                let diskURL = tempDownloadDir.appendingPathComponent("disk.img")
                if FileManager.default.fileExists(atPath: diskURL.path) {
                    try FileManager.default.copyItem(
                        at: diskURL,
                        to: tempVMDir.appendingPathComponent("disk.img")
                    )
                }
            }

            // Copy config and nvram files if they exist
            for file in ["config.json", "nvram.bin"] {
                let sourceURL = tempDownloadDir.appendingPathComponent(file)
                if FileManager.default.fileExists(atPath: sourceURL.path) {
                    try FileManager.default.copyItem(
                        at: sourceURL,
                        to: tempVMDir.appendingPathComponent(file)
                    )
                }
            }
        }

        // Only move to final location once everything is complete
        if FileManager.default.fileExists(atPath: vmDir.dir.path) {
            try FileManager.default.removeItem(at: URL(fileURLWithPath: vmDir.dir.path))
        }

        // Ensure parent directory exists
        try FileManager.default.createDirectory(
            at: URL(fileURLWithPath: vmDir.dir.path).deletingLastPathComponent(),
            withIntermediateDirectories: true)

        // Log the final destination
        Logger.info(
            "Moving files to VM directory",
            metadata: [
                "destination": vmDir.dir.path,
                "location": locationName ?? "default",
            ])

        // Move files to final location
        try FileManager.default.moveItem(at: tempVMDir, to: URL(fileURLWithPath: vmDir.dir.path))

        Logger.info("Download complete: Files extracted to \(vmDir.dir.path)")
        Logger.info(
            "Run 'lume run \(vmName)' to reduce the disk image file size by using macOS sparse file system"
        )
    }

    private func copyFromCache(manifest: Manifest, manifestId: String, to destination: URL)
        async throws
    {
        Logger.info("Copying from cache...")
        var diskPartSources: [(Int, URL)] = []
        var totalParts = 0
        var expectedTotalSize: UInt64 = 0

        // First identify disk parts and non-disk files
        for layer in manifest.layers {
            let cachedLayer = getCachedLayerPath(manifestId: manifestId, digest: layer.digest)

            if let partInfo = extractPartInfo(from: layer.mediaType) {
                let (partNum, total) = partInfo
                totalParts = total
                // Just store the reference to source instead of copying
                diskPartSources.append((partNum, cachedLayer))
                expectedTotalSize += UInt64(layer.size)
            } else {
                let fileName: String
                switch layer.mediaType {
                case "application/vnd.oci.image.layer.v1.tar":
                    fileName = "disk.img"
                case "application/vnd.oci.image.config.v1+json":
                    fileName = "config.json"
                case "application/octet-stream":
                    fileName = "nvram.bin"
                default:
                    continue
                }
                // Only non-disk files are copied
                try FileManager.default.copyItem(
                    at: cachedLayer,
                    to: destination.appendingPathComponent(fileName)
                )
            }
        }

        // Reassemble disk parts if needed
        if !diskPartSources.isEmpty {
            Logger.info("Reassembling disk image from cached parts (optimized storage)...")
            let outputURL = destination.appendingPathComponent("disk.img")
            FileManager.default.createFile(atPath: outputURL.path, contents: nil)
            let outputHandle = try FileHandle(forWritingTo: outputURL)
            defer { try? outputHandle.close() }

            var totalWritten: UInt64 = 0

            // Process parts in order, reading directly from cache
            for partNum in 1...totalParts {
                guard let (_, sourceURL) = diskPartSources.first(where: { $0.0 == partNum }) else {
                    throw PullError.missingPart(partNum)
                }

                // Read directly from the cached part
                let inputHandle = try FileHandle(forReadingFrom: sourceURL)
                defer { try? inputHandle.close() }

                // On low memory systems, be more aggressive with releasing memory
                let memoryConstrained = determineIfMemoryConstrained()
                var chunksProcessed = 0

                while let data = try inputHandle.read(upToCount: getOptimalChunkSize()) {
                    try autoreleasepool {
                        try outputHandle.write(contentsOf: data)
                        totalWritten += UInt64(data.count)

                        // Only log progress every 5% to reduce log noise
                        let progress: Double =
                            Double(totalWritten) / Double(expectedTotalSize) * 100
                        let roundedProgress = Int(progress / 5) * 5
                        if roundedProgress != Int(
                            (Double(totalWritten - UInt64(data.count)) / Double(expectedTotalSize)
                                * 100)
                                / 5) * 5
                        {
                            Logger.info("Reassembling disk image from cache: \(roundedProgress)%")
                        }

                        // Force more frequent autoreleases on memory-constrained systems
                        chunksProcessed += 1
                        if memoryConstrained && chunksProcessed % 10 == 0 {
                            try outputHandle.synchronize()
                        }
                    }
                }

                // Make sure we explicitly close handles after each part to free resources
                try? inputHandle.synchronize()
                try inputHandle.close()
            }

            // Verify final size
            let finalSize =
                try FileManager.default.attributesOfItem(atPath: outputURL.path)[.size] as? UInt64
                ?? 0
            Logger.info(
                "Final disk image size: \(ByteCountFormatter.string(fromByteCount: Int64(finalSize), countStyle: .file))"
            )
            Logger.info(
                "Expected size: \(ByteCountFormatter.string(fromByteCount: Int64(expectedTotalSize), countStyle: .file))"
            )

            if finalSize != expectedTotalSize {
                Logger.info(
                    "Warning: Final size (\(finalSize) bytes) differs from expected size (\(expectedTotalSize) bytes)"
                )
            }
        }

        Logger.info("Cache copy complete")
    }

    private func getToken(repository: String) async throws -> String {
        let url = URL(string: "https://\(self.registry)/token")!
            .appending(queryItems: [
                URLQueryItem(name: "service", value: self.registry),
                URLQueryItem(name: "scope", value: "repository:\(repository):pull"),
            ])

        let (data, _) = try await URLSession.shared.data(from: url)
        let json = try JSONSerialization.jsonObject(with: data) as? [String: Any]
        guard let token = json?["token"] as? String else {
            throw PullError.tokenFetchFailed
        }
        return token
    }

    private func fetchManifest(repository: String, tag: String, token: String) async throws -> (
        Manifest, String
    ) {
        var request = URLRequest(
            url: URL(string: "https://\(self.registry)/v2/\(repository)/manifests/\(tag)")!)
        request.addValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        request.addValue("application/vnd.oci.image.manifest.v1+json", forHTTPHeaderField: "Accept")

        let (data, response) = try await URLSession.shared.data(for: request)
        guard let httpResponse = response as? HTTPURLResponse,
            httpResponse.statusCode == 200,
            let digest = httpResponse.value(forHTTPHeaderField: "Docker-Content-Digest")
        else {
            throw PullError.manifestFetchFailed
        }

        let manifest = try JSONDecoder().decode(Manifest.self, from: data)
        return (manifest, digest)
    }

    private func downloadLayer(
        repository: String,
        digest: String,
        mediaType: String,
        token: String,
        to url: URL,
        maxRetries: Int = 5,
        progress: isolated ProgressTracker
    ) async throws {
        var lastError: Error?

        for attempt in 1...maxRetries {
            do {
                var request = URLRequest(
                    url: URL(string: "https://\(self.registry)/v2/\(repository)/blobs/\(digest)")!)
                request.addValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
                request.addValue(mediaType, forHTTPHeaderField: "Accept")
                request.timeoutInterval = 60

                // Configure session for better reliability
                let config = URLSessionConfiguration.default
                config.timeoutIntervalForRequest = 60
                config.timeoutIntervalForResource = 3600
                config.waitsForConnectivity = true
                config.httpMaximumConnectionsPerHost = 1

                let session = URLSession(configuration: config)

                let (tempURL, response) = try await session.download(for: request)
                guard let httpResponse = response as? HTTPURLResponse,
                    httpResponse.statusCode == 200
                else {
                    throw PullError.layerDownloadFailed(digest)
                }

                try FileManager.default.createDirectory(
                    at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
                try FileManager.default.moveItem(at: tempURL, to: url)
                progress.addProgress(Int64(httpResponse.expectedContentLength))
                return

            } catch {
                lastError = error
                if attempt < maxRetries {
                    let delay = Double(attempt) * 5
                    try await Task.sleep(nanoseconds: UInt64(delay * 1_000_000_000))
                }
            }
        }

        throw lastError ?? PullError.layerDownloadFailed(digest)
    }

    private func decompressGzipFile(at source: URL, to destination: URL) throws {
        Logger.info("Decompressing \(source.lastPathComponent)...")
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/gunzip")
        process.arguments = ["-c"]

        let inputPipe = Pipe()
        let outputPipe = Pipe()
        process.standardInput = inputPipe
        process.standardOutput = outputPipe

        try process.run()

        // Read and pipe the gzipped file in chunks to avoid memory issues
        let inputHandle = try FileHandle(forReadingFrom: source)
        let outputHandle = try FileHandle(forWritingTo: destination)
        defer {
            try? inputHandle.close()
            try? outputHandle.close()
        }

        // Create the output file
        FileManager.default.createFile(atPath: destination.path, contents: nil)

        // Process with optimal chunk size
        let chunkSize = getOptimalChunkSize()
        while let chunk = try inputHandle.read(upToCount: chunkSize) {
            try autoreleasepool {
                try inputPipe.fileHandleForWriting.write(contentsOf: chunk)

                // Read and write output in chunks as well
                while let decompressedChunk = try outputPipe.fileHandleForReading.read(
                    upToCount: chunkSize)
                {
                    try outputHandle.write(contentsOf: decompressedChunk)
                }
            }
        }

        try inputPipe.fileHandleForWriting.close()

        // Read any remaining output
        while let decompressedChunk = try outputPipe.fileHandleForReading.read(upToCount: chunkSize)
        {
            try autoreleasepool {
                try outputHandle.write(contentsOf: decompressedChunk)
            }
        }

        process.waitUntilExit()

        if process.terminationStatus != 0 {
            throw PullError.decompressionFailed(source.lastPathComponent)
        }

        // Verify the decompressed size
        let decompressedSize =
            try FileManager.default.attributesOfItem(atPath: destination.path)[.size] as? UInt64
            ?? 0
        Logger.info(
            "Decompressed size: \(ByteCountFormatter.string(fromByteCount: Int64(decompressedSize), countStyle: .file))"
        )
    }

    private func extractPartInfo(from mediaType: String) -> (partNum: Int, total: Int)? {
        let pattern = #"part\.number=(\d+);part\.total=(\d+)"#
        guard let regex = try? NSRegularExpression(pattern: pattern),
            let match = regex.firstMatch(
                in: mediaType,
                range: NSRange(mediaType.startIndex..., in: mediaType)
            ),
            let partNumRange = Range(match.range(at: 1), in: mediaType),
            let totalRange = Range(match.range(at: 2), in: mediaType),
            let partNum = Int(mediaType[partNumRange]),
            let total = Int(mediaType[totalRange])
        else {
            return nil
        }
        return (partNum, total)
    }

    private func listRepositories() async throws -> [String] {
        var request = URLRequest(
            url: URL(string: "https://\(registry)/v2/\(organization)/repositories/list")!)
        request.setValue("application/json", forHTTPHeaderField: "Accept")

        let (data, response) = try await URLSession.shared.data(for: request)
        guard let httpResponse = response as? HTTPURLResponse else {
            throw PullError.manifestFetchFailed
        }

        if httpResponse.statusCode == 404 {
            return []
        }

        guard httpResponse.statusCode == 200 else {
            throw PullError.manifestFetchFailed
        }

        let repoList = try JSONDecoder().decode(RepositoryList.self, from: data)
        return repoList.repositories
    }

    func getImages() async throws -> [CachedImage] {
        Logger.info("Scanning for cached images in \(cacheDirectory.path)")
        var images: [CachedImage] = []
        let orgDir = cacheDirectory.appendingPathComponent(organization)

        if FileManager.default.fileExists(atPath: orgDir.path) {
            let contents = try FileManager.default.contentsOfDirectory(atPath: orgDir.path)
            Logger.info("Found \(contents.count) items in cache directory")

            for item in contents {
                let itemPath = orgDir.appendingPathComponent(item)
                var isDirectory: ObjCBool = false

                guard
                    FileManager.default.fileExists(
                        atPath: itemPath.path, isDirectory: &isDirectory),
                    isDirectory.boolValue
                else { continue }

                // First try to read metadata file
                let metadataPath = itemPath.appendingPathComponent("metadata.json")
                if let metadataData = try? Data(contentsOf: metadataPath),
                    let metadata = try? JSONDecoder().decode(ImageMetadata.self, from: metadataData)
                {
                    Logger.info(
                        "Found metadata for image",
                        metadata: [
                            "image": metadata.image,
                            "manifest_id": metadata.manifestId,
                        ])
                    images.append(
                        CachedImage(
                            repository: metadata.image,
                            imageId: String(metadata.manifestId.prefix(12)),
                            manifestId: metadata.manifestId
                        ))
                    continue
                }

                // Fallback to checking manifest if metadata doesn't exist
                Logger.info("No metadata found for \(item), checking manifest")
                let manifestPath = itemPath.appendingPathComponent("manifest.json")
                guard FileManager.default.fileExists(atPath: manifestPath.path),
                    let manifestData = try? Data(contentsOf: manifestPath),
                    let manifest = try? JSONDecoder().decode(Manifest.self, from: manifestData)
                else {
                    Logger.info("No valid manifest found for \(item)")
                    continue
                }

                let manifestId = item

                // Verify the manifest ID matches
                let currentManifestId = getManifestIdentifier(manifest, manifestDigest: "")
                Logger.info(
                    "Manifest check",
                    metadata: [
                        "item": item,
                        "current_manifest_id": currentManifestId,
                        "matches": "\(currentManifestId == manifestId)",
                    ])
                if currentManifestId == manifestId {
                    // Skip if we can't determine the repository name
                    // This should be rare since we now save metadata during pull
                    Logger.info("Skipping image without metadata: \(item)")
                    continue
                }
            }
        } else {
            Logger.info("Cache directory does not exist")
        }

        Logger.info("Found \(images.count) cached images")
        return images.sorted {
            $0.repository == $1.repository ? $0.imageId < $1.imageId : $0.repository < $1.repository
        }
    }

    private func listRemoteImageTags(repository: String) async throws -> [String] {
        var request = URLRequest(
            url: URL(string: "https://\(registry)/v2/\(organization)/\(repository)/tags/list")!)
        request.setValue("application/json", forHTTPHeaderField: "Accept")

        let (data, response) = try await URLSession.shared.data(for: request)
        guard let httpResponse = response as? HTTPURLResponse else {
            throw PullError.manifestFetchFailed
        }

        if httpResponse.statusCode == 404 {
            return []
        }

        guard httpResponse.statusCode == 200 else {
            throw PullError.manifestFetchFailed
        }

        let repoTags = try JSONDecoder().decode(RepositoryTags.self, from: data)
        return repoTags.tags
    }

    // Determine appropriate chunk size based on available system memory on macOS
    private func getOptimalChunkSize() -> Int {
        // Try to get system memory info
        var stats = vm_statistics64_data_t()
        var size = mach_msg_type_number_t(
            MemoryLayout<vm_statistics64_data_t>.size / MemoryLayout<integer_t>.size)
        let hostPort = mach_host_self()

        let result = withUnsafeMutablePointer(to: &stats) { statsPtr in
            statsPtr.withMemoryRebound(to: integer_t.self, capacity: Int(size)) { ptr in
                host_statistics64(hostPort, HOST_VM_INFO64, ptr, &size)
            }
        }

        // Default to 512KB as a safe minimum
        let defaultChunkSize = 512 * 1024

        // If we can't get memory info, return conservative default
        guard result == KERN_SUCCESS else {
            return defaultChunkSize
        }

        // Calculate free memory in bytes using a fixed page size
        // Standard page size on macOS is 4KB or 16KB
        let pageSize = 4096  // Use a constant instead of vm_kernel_page_size
        let freeMemory = UInt64(stats.free_count) * UInt64(pageSize)

        // On very memory-constrained systems (< 1GB free), use the minimum
        if freeMemory < 1_073_741_824 {  // 1GB
            return defaultChunkSize
        }

        // For systems with adequate memory, use a smarter sizing approach:
        // - Use 0.1% of free memory, with limits
        let adaptiveSize = min(max(Int(freeMemory / 1000), defaultChunkSize), 2 * 1024 * 1024)
        return adaptiveSize
    }

    // Check if system is memory constrained for more aggressive memory management
    private func determineIfMemoryConstrained() -> Bool {
        var stats = vm_statistics64_data_t()
        var size = mach_msg_type_number_t(
            MemoryLayout<vm_statistics64_data_t>.size / MemoryLayout<integer_t>.size)
        let hostPort = mach_host_self()

        let result = withUnsafeMutablePointer(to: &stats) { statsPtr in
            statsPtr.withMemoryRebound(to: integer_t.self, capacity: Int(size)) { ptr in
                host_statistics64(hostPort, HOST_VM_INFO64, ptr, &size)
            }
        }

        guard result == KERN_SUCCESS else {
            // If we can't determine, assume constrained for safety
            return true
        }

        // Calculate free memory in bytes using a fixed page size
        // Standard page size on macOS is 4KB or 16KB
        let pageSize = 4096  // Use a constant instead of vm_kernel_page_size
        let freeMemory = UInt64(stats.free_count) * UInt64(pageSize)

        // Consider memory constrained if less than 2GB free
        return freeMemory < 2_147_483_648  // 2GB
    }
}
