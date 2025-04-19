import ArgumentParser
import Darwin
import Foundation
import Swift

// Define struct to decode relevant parts of config.json
struct OCIConfig: Codable {
    struct Annotations: Codable {
        let uncompressedSize: String?  // Use optional String

        enum CodingKeys: String, CodingKey {
            case uncompressedSize = "com.trycua.lume.disk.uncompressed_size"
        }
    }
    let annotations: Annotations?  // Optional annotations
}

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

    // Download speed tracking
    private var startTime: Date = Date()
    private var lastUpdateTime: Date = Date()
    private var lastUpdateBytes: Int64 = 0
    private var speedSamples: [Double] = []
    private var peakSpeed: Double = 0
    private var totalElapsedTime: TimeInterval = 0

    // Smoothing factor for speed calculation
    private var speedSmoothing: Double = 0.3
    private var smoothedSpeed: Double = 0

    func setTotal(_ total: Int64, files: Int) {
        totalBytes = total
        totalFiles = files
        startTime = Date()
        lastUpdateTime = startTime
        smoothedSpeed = 0
    }

    func addProgress(_ bytes: Int64) {
        downloadedBytes += bytes
        let now = Date()
        let elapsed = now.timeIntervalSince(lastUpdateTime)

        // Show first progress update immediately, then throttle updates
        let shouldUpdate = (downloadedBytes <= bytes) || (elapsed >= 0.5)

        if shouldUpdate {
            let currentSpeed = Double(downloadedBytes - lastUpdateBytes) / max(elapsed, 0.001)
            speedSamples.append(currentSpeed)

            // Cap samples array to prevent memory growth
            if speedSamples.count > 20 {
                speedSamples.removeFirst(speedSamples.count - 20)
            }

            // Update peak speed
            peakSpeed = max(peakSpeed, currentSpeed)

            // Apply exponential smoothing to the speed
            if smoothedSpeed == 0 {
                smoothedSpeed = currentSpeed
            } else {
                smoothedSpeed = speedSmoothing * currentSpeed + (1 - speedSmoothing) * smoothedSpeed
            }

            // Calculate average speed over the last few samples
            let recentAvgSpeed = calculateAverageSpeed()

            // Calculate overall average
            let totalElapsed = now.timeIntervalSince(startTime)
            let overallAvgSpeed = totalElapsed > 0 ? Double(downloadedBytes) / totalElapsed : 0

            let progress = Double(downloadedBytes) / Double(totalBytes)
            logSpeedProgress(
                current: progress,
                currentSpeed: currentSpeed,
                averageSpeed: recentAvgSpeed,
                smoothedSpeed: smoothedSpeed,
                overallSpeed: overallAvgSpeed,
                peakSpeed: peakSpeed,
                context: "Downloading Image"
            )

            // Update tracking variables
            lastUpdateTime = now
            lastUpdateBytes = downloadedBytes
            totalElapsedTime = totalElapsed
        }
    }

    private func calculateAverageSpeed() -> Double {
        guard !speedSamples.isEmpty else { return 0 }

        // Use weighted average giving more emphasis to recent samples
        var totalWeight = 0.0
        var weightedSum = 0.0

        let samples = speedSamples.suffix(min(8, speedSamples.count))
        for (index, speed) in samples.enumerated() {
            let weight = Double(index + 1)
            weightedSum += speed * weight
            totalWeight += weight
        }

        return totalWeight > 0 ? weightedSum / totalWeight : 0
    }

    func getDownloadStats() -> DownloadStats {
        let avgSpeed = totalElapsedTime > 0 ? Double(downloadedBytes) / totalElapsedTime : 0
        return DownloadStats(
            totalBytes: totalBytes,
            downloadedBytes: downloadedBytes,
            elapsedTime: totalElapsedTime,
            averageSpeed: avgSpeed,
            peakSpeed: peakSpeed
        )
    }

    private func logSpeedProgress(
        current: Double,
        currentSpeed: Double,
        averageSpeed: Double,
        smoothedSpeed: Double,
        overallSpeed: Double,
        peakSpeed: Double,
        context: String
    ) {
        let progressPercent = Int(current * 100)
        let currentSpeedStr = formatByteSpeed(currentSpeed)
        let avgSpeedStr = formatByteSpeed(averageSpeed)
        let peakSpeedStr = formatByteSpeed(peakSpeed)

        // Calculate ETA based on the smoothed speed which is more stable
        // This provides a more realistic estimate that doesn't fluctuate as much
        let remainingBytes = totalBytes - downloadedBytes
        let speedForEta = max(smoothedSpeed, averageSpeed * 0.8)  // Use the higher of smoothed or 80% of avg
        let etaSeconds = speedForEta > 0 ? Double(remainingBytes) / speedForEta : 0
        let etaStr = formatTimeRemaining(etaSeconds)

        let progressBar = createProgressBar(progress: current)

        print(
            "\r\(progressBar) \(progressPercent)% | Current: \(currentSpeedStr) | Avg: \(avgSpeedStr) | Peak: \(peakSpeedStr) | ETA: \(etaStr)     ",
            terminator: "")
        fflush(stdout)
    }

    private func createProgressBar(progress: Double, width: Int = 30) -> String {
        let completedWidth = Int(progress * Double(width))
        let remainingWidth = width - completedWidth

        let completed = String(repeating: "█", count: completedWidth)
        let remaining = String(repeating: "░", count: remainingWidth)

        return "[\(completed)\(remaining)]"
    }

    private func formatByteSpeed(_ bytesPerSecond: Double) -> String {
        let units = ["B/s", "KB/s", "MB/s", "GB/s"]
        var speed = bytesPerSecond
        var unitIndex = 0

        while speed > 1024 && unitIndex < units.count - 1 {
            speed /= 1024
            unitIndex += 1
        }

        return String(format: "%.1f %@", speed, units[unitIndex])
    }

    private func formatTimeRemaining(_ seconds: Double) -> String {
        if seconds.isNaN || seconds.isInfinite || seconds <= 0 {
            return "calculating..."
        }

        let hours = Int(seconds) / 3600
        let minutes = (Int(seconds) % 3600) / 60
        let secs = Int(seconds) % 60

        if hours > 0 {
            return String(format: "%d:%02d:%02d", hours, minutes, secs)
        } else {
            return String(format: "%d:%02d", minutes, secs)
        }
    }
}

struct DownloadStats {
    let totalBytes: Int64
    let downloadedBytes: Int64
    let elapsedTime: TimeInterval
    let averageSpeed: Double
    let peakSpeed: Double

    func formattedSummary() -> String {
        let bytesStr = ByteCountFormatter.string(fromByteCount: downloadedBytes, countStyle: .file)
        let avgSpeedStr = formatSpeed(averageSpeed)
        let peakSpeedStr = formatSpeed(peakSpeed)
        let timeStr = formatTime(elapsedTime)

        return """
            Download Statistics:
            - Total downloaded: \(bytesStr)
            - Elapsed time: \(timeStr)
            - Average speed: \(avgSpeedStr)
            - Peak speed: \(peakSpeedStr)
            """
    }

    private func formatSpeed(_ bytesPerSecond: Double) -> String {
        let formatter = ByteCountFormatter()
        formatter.countStyle = .file
        let bytesStr = formatter.string(fromByteCount: Int64(bytesPerSecond))
        return "\(bytesStr)/s"
    }

    private func formatTime(_ seconds: TimeInterval) -> String {
        let hours = Int(seconds) / 3600
        let minutes = (Int(seconds) % 3600) / 60
        let secs = Int(seconds) % 60

        if hours > 0 {
            return String(format: "%d hours, %d minutes, %d seconds", hours, minutes, secs)
        } else if minutes > 0 {
            return String(format: "%d minutes, %d seconds", minutes, secs)
        } else {
            return String(format: "%d seconds", secs)
        }
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
    private let cachingEnabled: Bool

    // Add the createProgressBar function here as a private method
    private func createProgressBar(progress: Double, width: Int = 30) -> String {
        let completedWidth = Int(progress * Double(width))
        let remainingWidth = width - completedWidth

        let completed = String(repeating: "█", count: completedWidth)
        let remaining = String(repeating: "░", count: remainingWidth)

        return "[\(completed)\(remaining)]"
    }

    init(registry: String, organization: String) {
        self.registry = registry
        self.organization = organization

        // Get cache directory from settings
        let cacheDir = SettingsManager.shared.getCacheDirectory()
        let expandedCacheDir = (cacheDir as NSString).expandingTildeInPath
        self.cacheDirectory = URL(fileURLWithPath: expandedCacheDir)
            .appendingPathComponent("ghcr")

        // Get caching enabled setting
        self.cachingEnabled = SettingsManager.shared.isCachingEnabled()

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
        // Skip cache validation if caching is disabled
        if !cachingEnabled {
            return false
        }

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
        // Skip saving manifest if caching is disabled
        if !cachingEnabled {
            return
        }

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
        // Skip saving metadata if caching is disabled
        if !cachingEnabled {
            return
        }

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
        // Skip cleanup if caching is disabled
        if !cachingEnabled {
            return
        }

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

    private func optimizeNetworkSettings() {
        // Set global URLSession configuration properties for better performance
        URLSessionConfiguration.default.httpMaximumConnectionsPerHost = 10
        URLSessionConfiguration.default.httpShouldUsePipelining = true
        URLSessionConfiguration.default.timeoutIntervalForResource = 3600

        // Pre-warm DNS resolution
        let preWarmTask = URLSession.shared.dataTask(with: URL(string: "https://\(self.registry)")!)
        preWarmTask.resume()
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
        let vmDir = try home.getVMDirectory(vmName, storage: locationName)

        // Optimize network early in the process
        optimizeNetworkSettings()

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

        // Check if caching is enabled and if we have a valid cached version
        Logger.info("Caching enabled: \(cachingEnabled)")
        if cachingEnabled && validateCache(manifest: manifest, manifestId: manifestId) {
            Logger.info("Using cached version of image")
            try await copyFromCache(manifest: manifest, manifestId: manifestId, to: tempVMDir)
        } else {
            // If caching is disabled, log it
            if !cachingEnabled {
                Logger.info("Caching is disabled, downloading fresh copy")
            } else {
                Logger.info("Cache miss or invalid cache, setting up new cache")
            }

            // Clean up old versions of this repository before setting up new cache if caching is enabled
            if cachingEnabled {
                try cleanupOldVersions(currentManifestId: manifestId, image: imageName)

                // Setup new cache directory
                try setupImageCache(manifestId: manifestId)
                // Save new manifest
                try saveManifest(manifest, manifestId: manifestId)

                // Save image metadata
                try saveImageMetadata(
                    image: imageName,
                    manifestId: manifestId
                )
            }

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
            await progress.setTotal(totalSize, files: totalFiles)

            // Process layers with limited concurrency
            Logger.info("Processing Image layers")
            Logger.info(
                "This may take several minutes depending on the image size and your internet connection. Please wait..."
            )

            // Add immediate progress indicator before starting downloads
            print(
                "[░░░░░░░░░░░░░░░░░░░░] 0% | Initializing downloads... | ETA: calculating...     ")
            fflush(stdout)

            var diskParts: [(Int, URL)] = []
            var totalParts = 0

            // Adaptive concurrency based on system capabilities
            let memoryConstrained = determineIfMemoryConstrained()
            let networkQuality = determineNetworkQuality()
            let maxConcurrentTasks = calculateOptimalConcurrency(
                memoryConstrained: memoryConstrained, networkQuality: networkQuality)

            Logger.info(
                "Using adaptive download configuration: Concurrency=\(maxConcurrentTasks), Memory-optimized=\(memoryConstrained)"
            )

            let counter = TaskCounter()

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
                            group.addTask { [self] in
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

                            group.addTask { [self] in
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
                                        progress: progress,
                                        manifestId: manifestId
                                    )

                                    // Cache the downloaded layer if caching is enabled
                                    if cachingEnabled {
                                        if FileManager.default.fileExists(atPath: cachedLayer.path)
                                        {
                                            try FileManager.default.removeItem(at: cachedLayer)
                                        }
                                        try FileManager.default.copyItem(
                                            at: partURL, to: cachedLayer)
                                    }
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
                        case "application/vnd.oci.image.layer.v1.tar",
                            "application/octet-stream+gzip":
                            outputURL = tempDownloadDir.appendingPathComponent("disk.img")
                        case "application/vnd.oci.image.config.v1+json":
                            outputURL = tempDownloadDir.appendingPathComponent("config.json")
                        case "application/octet-stream":
                            outputURL = tempDownloadDir.appendingPathComponent("nvram.bin")
                        default:
                            continue
                        }

                        group.addTask { [self] in
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
                                    progress: progress,
                                    manifestId: manifestId
                                )

                                // Cache the downloaded layer if caching is enabled
                                if cachingEnabled {
                                    if FileManager.default.fileExists(atPath: cachedLayer.path) {
                                        try FileManager.default.removeItem(at: cachedLayer)
                                    }
                                    try FileManager.default.copyItem(at: outputURL, to: cachedLayer)
                                }
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

            // Display download statistics
            let stats = await progress.getDownloadStats()
            Logger.info(stats.formattedSummary())

            // Parse config.json to get uncompressed size *before* reassembly
            let configURL = tempDownloadDir.appendingPathComponent("config.json")
            let uncompressedSize = getUncompressedSizeFromConfig(configPath: configURL)

            // Now also try to get disk size from VM config if OCI annotation not found
            var vmConfigDiskSize: UInt64? = nil
            if uncompressedSize == nil && FileManager.default.fileExists(atPath: configURL.path) {
                do {
                    let configData = try Data(contentsOf: configURL)
                    let decoder = JSONDecoder()
                    if let vmConfig = try? decoder.decode(VMConfig.self, from: configData) {
                        vmConfigDiskSize = vmConfig.diskSize
                        if let size = vmConfigDiskSize {
                            Logger.info("Found diskSize from VM config.json: \(size) bytes")
                        }
                    }
                } catch {
                    Logger.error("Failed to parse VM config.json for diskSize: \(error)")
                }
            }

            // Force explicit use
            if uncompressedSize != nil {
                Logger.info(
                    "Will use uncompressed size from annotation for sparse file: \(uncompressedSize!) bytes"
                )
            } else if vmConfigDiskSize != nil {
                Logger.info(
                    "Will use diskSize from VM config for sparse file: \(vmConfigDiskSize!) bytes")
            }

            // Handle disk parts if present
            if !diskParts.isEmpty {
                Logger.info("Reassembling disk image using sparse file technique...")
                let outputURL = tempVMDir.appendingPathComponent("disk.img")

                // Wrap setup in do-catch for better error reporting
                let outputHandle: FileHandle
                do {
                    // 1. Ensure parent directory exists
                    try FileManager.default.createDirectory(
                        at: outputURL.deletingLastPathComponent(), withIntermediateDirectories: true
                    )

                    // 2. Explicitly create the file first, removing old one if needed
                    if FileManager.default.fileExists(atPath: outputURL.path) {
                        try FileManager.default.removeItem(at: outputURL)
                    }
                    guard FileManager.default.createFile(atPath: outputURL.path, contents: nil)
                    else {
                        throw PullError.fileCreationFailed(outputURL.path)
                    }

                    // 3. Now open the handle for writing
                    outputHandle = try FileHandle(forWritingTo: outputURL)

                } catch {
                    // Catch errors during directory/file creation or handle opening
                    Logger.error(
                        "Failed during setup for disk image reassembly: \(error.localizedDescription)",
                        metadata: ["path": outputURL.path])
                    throw PullError.reassemblySetupFailed(
                        path: outputURL.path, underlyingError: error)
                }

                // Calculate expected size from the manifest layers (sum of compressed parts - for logging only now)
                let expectedCompressedTotalSize = UInt64(
                    manifest.layers.filter { extractPartInfo(from: $0.mediaType) != nil }.reduce(0)
                    { $0 + $1.size }
                )
                Logger.info(
                    "Total compressed parts size: \(ByteCountFormatter.string(fromByteCount: Int64(expectedCompressedTotalSize), countStyle: .file))"
                )

                // Calculate fallback size (sum of compressed parts)
                let _: UInt64 = diskParts.reduce(UInt64(0)) {
                    (acc: UInt64, element) -> UInt64 in
                    let fileSize =
                        (try? FileManager.default.attributesOfItem(atPath: element.1.path)[.size]
                            as? UInt64 ?? 0) ?? 0
                    return acc + fileSize
                }

                // Use: annotation size > VM config diskSize > fallback size
                let sizeForTruncate: UInt64
                if let size = uncompressedSize {
                    Logger.info("Using uncompressed size from annotation: \(size) bytes")
                    sizeForTruncate = size
                } else if let size = vmConfigDiskSize {
                    Logger.info("Using diskSize from VM config: \(size) bytes")
                    sizeForTruncate = size
                } else {
                    Logger.error(
                        "Missing both uncompressed size annotation and VM config diskSize for multi-part image."
                    )
                    throw PullError.missingUncompressedSizeAnnotation
                }

                defer { try? outputHandle.close() }

                // Set the file size without writing data (creates a sparse file)
                try outputHandle.truncate(atOffset: sizeForTruncate)

                // Verify the sparse file was created with the correct size
                let initialSize =
                    (try? FileManager.default.attributesOfItem(atPath: outputURL.path)[.size]
                        as? UInt64) ?? 0
                Logger.info(
                    "Sparse file initialized with size: \(ByteCountFormatter.string(fromByteCount: Int64(initialSize), countStyle: .file))"
                )

                // Add a simple test pattern at the beginning and end of the file to verify it's writable
                try outputHandle.seek(toOffset: 0)
                let testPattern = "LUME_TEST_PATTERN".data(using: .utf8)!
                try outputHandle.write(contentsOf: testPattern)

                try outputHandle.seek(toOffset: sizeForTruncate - UInt64(testPattern.count))
                try outputHandle.write(contentsOf: testPattern)
                try outputHandle.synchronize()

                Logger.info("Test patterns written to sparse file. File is ready for writing.")

                var reassemblyProgressLogger = ProgressLogger(threshold: 0.05)
                var currentOffset: UInt64 = 0  // Track position in the final *decompressed* file

                for partNum in 1...totalParts {
                    // Find the original layer info for this part number
                    guard
                        let layer = manifest.layers.first(where: { layer in
                            if let info = extractPartInfo(from: layer.mediaType) {
                                return info.partNum == partNum
                            }
                            return false
                        }),
                        let (_, partURL) = diskParts.first(where: { $0.0 == partNum })
                    else {
                        throw PullError.missingPart(partNum)
                    }
                    let layerMediaType = layer.mediaType  // Extract mediaType here

                    Logger.info(
                        "Processing part \(partNum) of \(totalParts): \(partURL.lastPathComponent)")

                    let inputHandle = try FileHandle(forReadingFrom: partURL)
                    defer {
                        try? inputHandle.close()
                        // Clean up temp downloaded part if not from cache
                        if !partURL.path.contains(cacheDirectory.path) {
                            try? FileManager.default.removeItem(at: partURL)
                        }
                    }

                    // Seek to the correct offset in the output sparse file
                    try outputHandle.seek(toOffset: currentOffset)

                    if let decompressCmd = getDecompressionCommand(for: layerMediaType) {  // Use extracted mediaType
                        Logger.info(
                            "Decompressing part \(partNum) with media type: \(layerMediaType)")

                        // Handle Apple Archive format
                        let toolPath = String(decompressCmd.dropFirst("apple_archive:".count))
                        let tempOutputPath = FileManager.default.temporaryDirectory
                            .appendingPathComponent(UUID().uuidString)

                        // Check input file size before decompression
                        let inputFileSize =
                            (try? FileManager.default.attributesOfItem(atPath: partURL.path)[.size]
                                as? UInt64) ?? 0
                        Logger.info(
                            "Part \(partNum) input size: \(ByteCountFormatter.string(fromByteCount: Int64(inputFileSize), countStyle: .file))"
                        )

                        // Create a process that decompresses to a temporary file
                        let process = Process()
                        process.executableURL = URL(fileURLWithPath: toolPath)
                        process.arguments = [
                            "extract", "-i", partURL.path, "-o", tempOutputPath.path,
                        ]

                        // Add error output capture
                        let errorPipe = Pipe()
                        process.standardError = errorPipe

                        Logger.info(
                            "Decompressing Apple Archive format with: \(toolPath) \(process.arguments?.joined(separator: " ") ?? "")"
                        )
                        try process.run()
                        process.waitUntilExit()

                        // Check error output if any
                        let errorData = errorPipe.fileHandleForReading.readDataToEndOfFile()
                        if !errorData.isEmpty,
                            let errorString = String(data: errorData, encoding: .utf8)
                        {
                            Logger.error("Decompression error output: \(errorString)")
                        }

                        if process.terminationStatus != 0 {
                            Logger.error(
                                "Apple Archive decompression failed with status: \(process.terminationStatus), falling back to direct copy"
                            )
                            // Fall back to direct copying (uncompressed)
                            Logger.info("Copying part \(partNum) directly without decompression...")
                            try outputHandle.seek(toOffset: currentOffset)

                            let inputHandle = try FileHandle(forReadingFrom: partURL)
                            defer { try? inputHandle.close() }

                            var bytesWritten: UInt64 = 0
                            let chunkSize = 1024 * 1024  // 1MB chunks
                            var chunkCount = 0

                            while true {
                                let data = autoreleasepool {
                                    try! inputHandle.read(upToCount: chunkSize) ?? Data()
                                }
                                if data.isEmpty { break }

                                try outputHandle.write(contentsOf: data)
                                bytesWritten += UInt64(data.count)
                                chunkCount += 1

                                // Update progress
                                let totalProgress =
                                    Double(currentOffset + bytesWritten)
                                    / Double(expectedCompressedTotalSize)
                                let progressBar = createProgressBar(
                                    progress: totalProgress, width: 30)
                                let progressPercent = Int(totalProgress * 100)
                                let currentSpeed =
                                    ByteCountFormatter.string(
                                        fromByteCount: Int64(Double(bytesWritten) / 0.5),
                                        countStyle: .file) + "/s"

                                print(
                                    "\r\(progressBar) \(progressPercent)% | Speed: \(currentSpeed) | Part \(partNum) | \(ByteCountFormatter.string(fromByteCount: Int64(currentOffset + bytesWritten), countStyle: .file))     ",
                                    terminator: "")
                                fflush(stdout)

                                // Also log to the progress logger for consistency
                                reassemblyProgressLogger.logProgress(
                                    current: totalProgress,
                                    context: "Direct copying")
                            }

                            Logger.info(
                                "Part \(partNum) - Direct copy: wrote \(chunkCount) chunks, total bytes: \(ByteCountFormatter.string(fromByteCount: Int64(bytesWritten), countStyle: .file))"
                            )
                            currentOffset += bytesWritten
                            continue
                        }

                        // Check if the output file exists and has content
                        let outputExists = FileManager.default.fileExists(
                            atPath: tempOutputPath.path)
                        let outputFileSize =
                            outputExists
                            ? ((try? FileManager.default.attributesOfItem(
                                atPath: tempOutputPath.path)[
                                    .size] as? UInt64) ?? 0) : 0
                        Logger.info(
                            "Part \(partNum) - Decompressed output exists: \(outputExists), size: \(ByteCountFormatter.string(fromByteCount: Int64(outputFileSize), countStyle: .file))"
                        )

                        // If decompression produced an empty file, fall back to direct copy
                        if outputFileSize == 0 {
                            Logger.info(
                                "Decompression resulted in empty file, falling back to direct copy for part \(partNum)"
                            )
                            try? FileManager.default.removeItem(at: tempOutputPath)

                            // Fall back to direct copying (uncompressed)
                            Logger.info("Copying part \(partNum) directly without decompression...")
                            try outputHandle.seek(toOffset: currentOffset)

                            let inputHandle = try FileHandle(forReadingFrom: partURL)
                            defer { try? inputHandle.close() }

                            var bytesWritten: UInt64 = 0
                            let chunkSize = 1024 * 1024  // 1MB chunks
                            var chunkCount = 0

                            while true {
                                let data = autoreleasepool {
                                    try! inputHandle.read(upToCount: chunkSize) ?? Data()
                                }
                                if data.isEmpty { break }

                                try outputHandle.write(contentsOf: data)
                                bytesWritten += UInt64(data.count)
                                chunkCount += 1

                                // Update progress
                                let totalProgress =
                                    Double(currentOffset + bytesWritten)
                                    / Double(expectedCompressedTotalSize)
                                let progressBar = createProgressBar(
                                    progress: totalProgress, width: 30)
                                let progressPercent = Int(totalProgress * 100)
                                let currentSpeed =
                                    ByteCountFormatter.string(
                                        fromByteCount: Int64(Double(bytesWritten) / 0.5),
                                        countStyle: .file) + "/s"

                                print(
                                    "\r\(progressBar) \(progressPercent)% | Speed: \(currentSpeed) | Part \(partNum) | \(ByteCountFormatter.string(fromByteCount: Int64(currentOffset + bytesWritten), countStyle: .file))     ",
                                    terminator: "")
                                fflush(stdout)

                                // Also log to the progress logger for consistency
                                reassemblyProgressLogger.logProgress(
                                    current: totalProgress,
                                    context: "Direct copying")
                            }

                            Logger.info(
                                "Part \(partNum) - Direct copy: wrote \(chunkCount) chunks, total bytes: \(ByteCountFormatter.string(fromByteCount: Int64(bytesWritten), countStyle: .file))"
                            )
                            currentOffset += bytesWritten
                            continue
                        }

                        // Read the decompressed file and write to our output
                        let tempInputHandle = try FileHandle(forReadingFrom: tempOutputPath)
                        defer {
                            try? tempInputHandle.close()
                            try? FileManager.default.removeItem(at: tempOutputPath)
                        }

                        // Read decompressed data in chunks and write to sparse file
                        var partDecompressedSize: UInt64 = 0
                        let chunkSize = 1024 * 1024  // 1MB chunks
                        var chunkCount = 0

                        while true {
                            let data = autoreleasepool {  // Help manage memory with large files
                                try! tempInputHandle.read(upToCount: chunkSize) ?? Data()
                            }
                            if data.isEmpty { break }  // End of stream

                            try outputHandle.write(contentsOf: data)
                            partDecompressedSize += UInt64(data.count)
                            chunkCount += 1

                            // Update progress based on decompressed size written
                            let totalProgress =
                                Double(currentOffset + partDecompressedSize)
                                / Double(expectedCompressedTotalSize)
                            reassemblyProgressLogger.logProgress(
                                current: totalProgress,
                                context: "Reassembling")
                        }

                        Logger.info(
                            "Part \(partNum) - Wrote \(chunkCount) chunks, total bytes: \(ByteCountFormatter.string(fromByteCount: Int64(partDecompressedSize), countStyle: .file))"
                        )
                        currentOffset += partDecompressedSize  // Advance offset by decompressed size
                    } else {
                        // No decompression command available, try direct copy
                        Logger.info(
                            "Copying part \(partNum) directly..."
                        )
                        try outputHandle.seek(toOffset: currentOffset)

                        let inputHandle = try FileHandle(forReadingFrom: partURL)
                        defer { try? inputHandle.close() }

                        // Get part size
                        let partSize =
                            (try? FileManager.default.attributesOfItem(atPath: partURL.path)[.size]
                                as? UInt64) ?? 0
                        Logger.info(
                            "Direct copy of part \(partNum) with size: \(ByteCountFormatter.string(fromByteCount: Int64(partSize), countStyle: .file))"
                        )

                        var bytesWritten: UInt64 = 0
                        let chunkSize = 1024 * 1024  // 1MB chunks
                        var chunkCount = 0

                        while true {
                            let data = autoreleasepool {
                                try! inputHandle.read(upToCount: chunkSize) ?? Data()
                            }
                            if data.isEmpty { break }

                            try outputHandle.write(contentsOf: data)
                            bytesWritten += UInt64(data.count)
                            chunkCount += 1

                            // Update progress
                            let totalProgress =
                                Double(currentOffset + bytesWritten)
                                / Double(expectedCompressedTotalSize)
                            reassemblyProgressLogger.logProgress(
                                current: totalProgress,
                                context: "Direct copying")
                        }

                        Logger.info(
                            "Part \(partNum) - Direct copy: wrote \(chunkCount) chunks, total bytes: \(ByteCountFormatter.string(fromByteCount: Int64(bytesWritten), countStyle: .file))"
                        )
                        currentOffset += bytesWritten
                    }

                    // Ensure data is written before processing next part (optional but safer)
                    try outputHandle.synchronize()
                }

                // Finalize progress, close handle (done by defer)
                reassemblyProgressLogger.logProgress(current: 1.0, context: "Reassembly Complete")
                Logger.info("")  // Newline

                // Ensure output handle is closed before post-processing
                try outputHandle.close()

                // Verify final size
                let finalSize =
                    (try? FileManager.default.attributesOfItem(atPath: outputURL.path)[.size]
                        as? UInt64) ?? 0
                Logger.info(
                    "Final disk image size from cache (before sparse file optimization): \(ByteCountFormatter.string(fromByteCount: Int64(finalSize), countStyle: .file))"
                )

                if finalSize != sizeForTruncate {
                    Logger.info(
                        "Warning: Final reported size (\(finalSize) bytes) differs from expected size (\(sizeForTruncate) bytes), but this doesn't affect functionality"
                    )
                }

                // Decompress the assembled disk image if it's in LZFSE compressed format
                Logger.info(
                    "Checking if disk image is LZFSE compressed and decompressing if needed...")
                decompressLZFSEImage(inputPath: outputURL.path)

                // Create a properly formatted disk image
                Logger.info("Converting assembled data to proper disk image format...")

                // Get actual disk usage of the assembled file
                let assembledUsage = getActualDiskUsage(path: outputURL.path)
                let bufferBytes: UInt64 = 2 * 1024 * 1024 * 1024  // 2GB buffer
                let requiredSpace = assembledUsage + bufferBytes

                // Check available disk space in the destination directory
                let fileManager = FileManager.default
                let availableSpace =
                    try? fileManager.attributesOfFileSystem(
                        forPath: outputURL.deletingLastPathComponent().path)[.systemFreeSize]
                    as? UInt64

                if let available = availableSpace, available < requiredSpace {
                    Logger.error(
                        "Insufficient disk space to convert disk image format. Skipping conversion.",
                        metadata: [
                            "available": ByteCountFormatter.string(
                                fromByteCount: Int64(available), countStyle: .file),
                            "required": ByteCountFormatter.string(
                                fromByteCount: Int64(requiredSpace), countStyle: .file),
                        ]
                    )
                } else {
                    // Prioritize SPARSE format for better sparse file handling
                    Logger.info("Attempting conversion to SPARSE format...")
                    let process = Process()
                    process.executableURL = URL(fileURLWithPath: "/usr/bin/hdiutil")
                    process.arguments = [
                        "convert",
                        outputURL.path,  // Source: our assembled file
                        "-format", "SPARSE",  // Format: SPARSE (best for sparse images)
                        "-o", outputURL.path,  // Output: overwrite with converted image
                    ]

                    let errorPipe = Pipe()
                    process.standardError = errorPipe
                    process.standardOutput = errorPipe

                    try process.run()
                    process.waitUntilExit()

                    // Check for errors
                    let outputData = errorPipe.fileHandleForReading.readDataToEndOfFile()
                    if !outputData.isEmpty,
                        let outputString = String(data: outputData, encoding: .utf8)
                    {
                        Logger.info("hdiutil output: \(outputString)")
                    }

                    if process.terminationStatus == 0 {
                        // Find the potentially renamed formatted file
                        let formattedFile = findFormattedFile(tempFormatted: outputURL) ?? outputURL
                        // If the output path is different, remove the original and move the new one
                        if formattedFile.path != outputURL.path {
                            try? FileManager.default.removeItem(at: outputURL)
                            try FileManager.default.moveItem(at: formattedFile, to: outputURL)
                        }
                        Logger.info("Successfully converted disk image to proper format (SPARSE)")
                    } else {
                        Logger.error(
                            "Failed to convert disk image to SPARSE format. VM might not start properly."
                        )
                        // If SPARSE failed, maybe try UDRW as a last resort?
                        // For now, we'll just log the error.
                    }
                }

                Logger.info("Disk image reassembly completed")
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
            "Note: Actual disk usage is significantly lower than reported size due to macOS sparse file system"
        )
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

        // First identify disk parts and non-disk files
        for layer in manifest.layers {
            let cachedLayer = getCachedLayerPath(manifestId: manifestId, digest: layer.digest)

            if let partInfo = extractPartInfo(from: layer.mediaType) {
                let (partNum, total) = partInfo
                totalParts = total
                // Just store the reference to source instead of copying
                diskPartSources.append((partNum, cachedLayer))
            } else {
                let fileName: String
                switch layer.mediaType {
                case "application/vnd.oci.image.layer.v1.tar", "application/octet-stream+gzip":
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
            // Get the uncompressed size from cached config
            let configDigest = manifest.config?.digest
            let cachedConfigPath =
                configDigest != nil
                ? getCachedLayerPath(manifestId: manifestId, digest: configDigest!) : nil
            let uncompressedSize = cachedConfigPath.flatMap {
                getUncompressedSizeFromConfig(configPath: $0)
            }

            // Try to get disk size from VM config if OCI annotation not found
            var vmConfigDiskSize: UInt64? = nil
            if uncompressedSize == nil {
                // Find config.json in the copied files
                let vmConfigPath = destination.appendingPathComponent("config.json")
                if FileManager.default.fileExists(atPath: vmConfigPath.path) {
                    do {
                        let configData = try Data(contentsOf: vmConfigPath)
                        let decoder = JSONDecoder()
                        if let vmConfig = try? decoder.decode(VMConfig.self, from: configData) {
                            vmConfigDiskSize = vmConfig.diskSize
                            if let size = vmConfigDiskSize {
                                Logger.info(
                                    "Found diskSize from cached VM config.json: \(size) bytes")
                            }
                        }
                    } catch {
                        Logger.error("Failed to parse cached VM config.json for diskSize: \(error)")
                    }
                }
            }

            // Force explicit use
            if uncompressedSize != nil {
                Logger.info(
                    "Will use uncompressed size from annotation for sparse file: \(uncompressedSize!) bytes"
                )
            } else if vmConfigDiskSize != nil {
                Logger.info(
                    "Will use diskSize from VM config for sparse file: \(vmConfigDiskSize!) bytes")
            }

            Logger.info(
                "Reassembling disk image from cached parts using sparse file technique..."
            )
            let outputURL = destination.appendingPathComponent("disk.img")

            // Wrap setup in do-catch for better error reporting
            let outputHandle: FileHandle
            do {
                // 1. Ensure parent directory exists
                try FileManager.default.createDirectory(
                    at: outputURL.deletingLastPathComponent(), withIntermediateDirectories: true)

                // 2. Explicitly create the file first, removing old one if needed
                if FileManager.default.fileExists(atPath: outputURL.path) {
                    try FileManager.default.removeItem(at: outputURL)
                }
                guard FileManager.default.createFile(atPath: outputURL.path, contents: nil) else {
                    throw PullError.fileCreationFailed(outputURL.path)
                }

                // 3. Now open the handle for writing
                outputHandle = try FileHandle(forWritingTo: outputURL)

            } catch {
                // Catch errors during directory/file creation or handle opening
                Logger.error(
                    "Failed during setup for disk image reassembly: \(error.localizedDescription)",
                    metadata: ["path": outputURL.path])
                throw PullError.reassemblySetupFailed(path: outputURL.path, underlyingError: error)
            }

            // Calculate expected total size from the cached files
            let expectedTotalSize: UInt64 = diskPartSources.reduce(UInt64(0)) {
                (acc: UInt64, element) -> UInt64 in
                let fileSize =
                    (try? FileManager.default.attributesOfItem(atPath: element.1.path)[.size]
                        as? UInt64 ?? 0) ?? 0
                return acc + fileSize
            }
            Logger.info(
                "Expected download size from cache: \(ByteCountFormatter.string(fromByteCount: Int64(expectedTotalSize), countStyle: .file)) (actual disk usage will be lower)"
            )

            var reassemblyProgressLogger = ProgressLogger(threshold: 0.05)
            var currentOffset: UInt64 = 0  // Track position in the final *decompressed* file

            for partNum in 1...totalParts {
                // Find the original layer info for this part number
                guard
                    let layer = manifest.layers.first(where: { layer in
                        if let info = extractPartInfo(from: layer.mediaType) {
                            return info.partNum == partNum
                        }
                        return false
                    }),
                    let (_, sourceURL) = diskPartSources.first(where: { $0.0 == partNum })
                else {
                    throw PullError.missingPart(partNum)
                }
                let layerMediaType = layer.mediaType  // Extract mediaType here

                Logger.info(
                    "Processing part \(partNum) of \(totalParts) from cache: \(sourceURL.lastPathComponent)"
                )

                let inputHandle = try FileHandle(forReadingFrom: sourceURL)
                defer { try? inputHandle.close() }

                // Seek to the correct offset in the output sparse file
                try outputHandle.seek(toOffset: currentOffset)

                if let decompressCmd = getDecompressionCommand(for: layerMediaType) {  // Use extracted mediaType
                    Logger.info("Decompressing part \(partNum) with media type: \(layerMediaType)")

                    // Handle Apple Archive format
                    let toolPath = String(decompressCmd.dropFirst("apple_archive:".count))
                    let tempOutputPath = FileManager.default.temporaryDirectory
                        .appendingPathComponent(UUID().uuidString)

                    // Check input file size before decompression
                    let inputFileSize =
                        (try? FileManager.default.attributesOfItem(atPath: sourceURL.path)[.size]
                            as? UInt64) ?? 0
                    Logger.info(
                        "Part \(partNum) input size: \(ByteCountFormatter.string(fromByteCount: Int64(inputFileSize), countStyle: .file))"
                    )

                    // Create a process that decompresses to a temporary file
                    let process = Process()
                    process.executableURL = URL(fileURLWithPath: toolPath)
                    process.arguments = [
                        "extract", "-i", sourceURL.path, "-o", tempOutputPath.path,
                    ]

                    // Add error output capture
                    let errorPipe = Pipe()
                    process.standardError = errorPipe

                    Logger.info(
                        "Decompressing Apple Archive format with: \(toolPath) \(process.arguments?.joined(separator: " ") ?? "")"
                    )
                    try process.run()
                    process.waitUntilExit()

                    // Check error output if any
                    let errorData = errorPipe.fileHandleForReading.readDataToEndOfFile()
                    if !errorData.isEmpty,
                        let errorString = String(data: errorData, encoding: .utf8)
                    {
                        Logger.error("Decompression error output: \(errorString)")
                    }

                    if process.terminationStatus != 0 {
                        Logger.error(
                            "Apple Archive decompression failed with status: \(process.terminationStatus), falling back to direct copy"
                        )
                        // Fall back to direct copying (uncompressed)
                        Logger.info("Copying part \(partNum) directly without decompression...")
                        try outputHandle.seek(toOffset: currentOffset)

                        let inputHandle = try FileHandle(forReadingFrom: sourceURL)
                        defer { try? inputHandle.close() }

                        var bytesWritten: UInt64 = 0
                        let chunkSize = 1024 * 1024  // 1MB chunks
                        var chunkCount = 0

                        while true {
                            let data = autoreleasepool {
                                try! inputHandle.read(upToCount: chunkSize) ?? Data()
                            }
                            if data.isEmpty { break }

                            try outputHandle.write(contentsOf: data)
                            bytesWritten += UInt64(data.count)
                            chunkCount += 1

                            // Update progress
                            let totalProgress =
                                Double(currentOffset + bytesWritten) / Double(expectedTotalSize)
                            let progressBar = createProgressBar(progress: totalProgress, width: 30)
                            let progressPercent = Int(totalProgress * 100)
                            let currentSpeed =
                                ByteCountFormatter.string(
                                    fromByteCount: Int64(Double(bytesWritten) / 0.5),
                                    countStyle: .file) + "/s"

                            print(
                                "\r\(progressBar) \(progressPercent)% | Speed: \(currentSpeed) | Part \(partNum) | \(ByteCountFormatter.string(fromByteCount: Int64(currentOffset + bytesWritten), countStyle: .file))     ",
                                terminator: "")
                            fflush(stdout)

                            // Also log to the progress logger for consistency
                            reassemblyProgressLogger.logProgress(
                                current: totalProgress,
                                context: "Direct copying")
                        }

                        Logger.info(
                            "Part \(partNum) - Direct copy: wrote \(chunkCount) chunks, total bytes: \(ByteCountFormatter.string(fromByteCount: Int64(bytesWritten), countStyle: .file))"
                        )
                        currentOffset += bytesWritten
                        continue
                    }

                    // Check if the output file exists and has content
                    let outputExists = FileManager.default.fileExists(atPath: tempOutputPath.path)
                    let outputFileSize =
                        outputExists
                        ? ((try? FileManager.default.attributesOfItem(atPath: tempOutputPath.path)[
                            .size] as? UInt64) ?? 0) : 0
                    Logger.info(
                        "Part \(partNum) - Decompressed output exists: \(outputExists), size: \(ByteCountFormatter.string(fromByteCount: Int64(outputFileSize), countStyle: .file))"
                    )

                    // If decompression produced an empty file, fall back to direct copy
                    if outputFileSize == 0 {
                        Logger.info(
                            "Decompression resulted in empty file, falling back to direct copy for part \(partNum)"
                        )
                        try? FileManager.default.removeItem(at: tempOutputPath)

                        // Fall back to direct copying (uncompressed)
                        Logger.info("Copying part \(partNum) directly without decompression...")
                        try outputHandle.seek(toOffset: currentOffset)

                        let inputHandle = try FileHandle(forReadingFrom: sourceURL)
                        defer { try? inputHandle.close() }

                        var bytesWritten: UInt64 = 0
                        let chunkSize = 1024 * 1024  // 1MB chunks
                        var chunkCount = 0

                        while true {
                            let data = autoreleasepool {
                                try! inputHandle.read(upToCount: chunkSize) ?? Data()
                            }
                            if data.isEmpty { break }

                            try outputHandle.write(contentsOf: data)
                            bytesWritten += UInt64(data.count)
                            chunkCount += 1

                            // Update progress
                            let totalProgress =
                                Double(currentOffset + bytesWritten) / Double(expectedTotalSize)
                            let progressBar = createProgressBar(progress: totalProgress, width: 30)
                            let progressPercent = Int(totalProgress * 100)
                            let currentSpeed =
                                ByteCountFormatter.string(
                                    fromByteCount: Int64(Double(bytesWritten) / 0.5),
                                    countStyle: .file) + "/s"

                            print(
                                "\r\(progressBar) \(progressPercent)% | Speed: \(currentSpeed) | Part \(partNum) | \(ByteCountFormatter.string(fromByteCount: Int64(currentOffset + bytesWritten), countStyle: .file))     ",
                                terminator: "")
                            fflush(stdout)

                            // Also log to the progress logger for consistency
                            reassemblyProgressLogger.logProgress(
                                current: totalProgress,
                                context: "Direct copying")
                        }

                        Logger.info(
                            "Part \(partNum) - Direct copy: wrote \(chunkCount) chunks, total bytes: \(ByteCountFormatter.string(fromByteCount: Int64(bytesWritten), countStyle: .file))"
                        )
                        currentOffset += bytesWritten
                        continue
                    }

                    // Read the decompressed file and write to our output
                    let tempInputHandle = try FileHandle(forReadingFrom: tempOutputPath)
                    defer {
                        try? tempInputHandle.close()
                        try? FileManager.default.removeItem(at: tempOutputPath)
                    }

                    // Read decompressed data in chunks and write to sparse file
                    var partDecompressedSize: UInt64 = 0
                    let chunkSize = 1024 * 1024  // 1MB chunks
                    var chunkCount = 0

                    while true {
                        let data = autoreleasepool {  // Help manage memory with large files
                            try! tempInputHandle.read(upToCount: chunkSize) ?? Data()
                        }
                        if data.isEmpty { break }  // End of stream

                        try outputHandle.write(contentsOf: data)
                        partDecompressedSize += UInt64(data.count)
                        chunkCount += 1

                        // Update progress based on decompressed size written
                        let totalProgress =
                            Double(currentOffset + partDecompressedSize)
                            / Double(expectedTotalSize)
                        reassemblyProgressLogger.logProgress(
                            current: totalProgress,
                            context: "Reassembling")
                    }

                    Logger.info(
                        "Part \(partNum) - Wrote \(chunkCount) chunks, total bytes: \(ByteCountFormatter.string(fromByteCount: Int64(partDecompressedSize), countStyle: .file))"
                    )
                    currentOffset += partDecompressedSize  // Advance offset by decompressed size
                } else {
                    // No decompression command available, try direct copy
                    Logger.info(
                        "Copying part \(partNum) directly..."
                    )
                    try outputHandle.seek(toOffset: currentOffset)

                    let inputHandle = try FileHandle(forReadingFrom: sourceURL)
                    defer { try? inputHandle.close() }

                    // Get part size
                    let partSize =
                        (try? FileManager.default.attributesOfItem(atPath: sourceURL.path)[.size]
                            as? UInt64) ?? 0
                    Logger.info(
                        "Direct copy of part \(partNum) with size: \(ByteCountFormatter.string(fromByteCount: Int64(partSize), countStyle: .file))"
                    )

                    var bytesWritten: UInt64 = 0
                    let chunkSize = 1024 * 1024  // 1MB chunks
                    var chunkCount = 0

                    while true {
                        let data = autoreleasepool {
                            try! inputHandle.read(upToCount: chunkSize) ?? Data()
                        }
                        if data.isEmpty { break }

                        try outputHandle.write(contentsOf: data)
                        bytesWritten += UInt64(data.count)
                        chunkCount += 1

                        // Update progress
                        let totalProgress =
                            Double(currentOffset + bytesWritten)
                            / Double(expectedTotalSize)
                        reassemblyProgressLogger.logProgress(
                            current: totalProgress,
                            context: "Direct copying")
                    }

                    Logger.info(
                        "Part \(partNum) - Direct copy: wrote \(chunkCount) chunks, total bytes: \(ByteCountFormatter.string(fromByteCount: Int64(bytesWritten), countStyle: .file))"
                    )
                    currentOffset += bytesWritten
                }

                // Ensure data is written before processing next part (optional but safer)
                try outputHandle.synchronize()
            }

            // Finalize progress, close handle (done by defer)
            reassemblyProgressLogger.logProgress(current: 1.0, context: "Reassembly Complete")
            Logger.info("")  // Newline

            // Ensure output handle is closed before post-processing
            try outputHandle.close()

            // Verify final size
            let finalSize =
                (try? FileManager.default.attributesOfItem(atPath: outputURL.path)[.size]
                    as? UInt64) ?? 0
            Logger.info(
                "Final disk image size from cache (before sparse file optimization): \(ByteCountFormatter.string(fromByteCount: Int64(finalSize), countStyle: .file))"
            )

            if finalSize != expectedTotalSize {
                Logger.info(
                    "Warning: Final reported size (\(finalSize) bytes) differs from expected size (\(expectedTotalSize) bytes), but this doesn't affect functionality"
                )
            }

            // Decompress the assembled disk image if it's in LZFSE compressed format
            Logger.info("Checking if disk image is LZFSE compressed and decompressing if needed...")
            decompressLZFSEImage(inputPath: outputURL.path)

            // Create a properly formatted disk image
            Logger.info("Converting assembled data to proper disk image format...")

            // Get actual disk usage of the assembled file
            let assembledUsage = getActualDiskUsage(path: outputURL.path)
            let bufferBytes: UInt64 = 2 * 1024 * 1024 * 1024  // 2GB buffer
            let requiredSpace = assembledUsage + bufferBytes

            // Check available disk space in the destination directory
            let fileManager = FileManager.default
            let availableSpace =
                try? fileManager.attributesOfFileSystem(
                    forPath: outputURL.deletingLastPathComponent().path)[.systemFreeSize] as? UInt64

            if let available = availableSpace, available < requiredSpace {
                Logger.error(
                    "Insufficient disk space to convert disk image format. Skipping conversion.",
                    metadata: [
                        "available": ByteCountFormatter.string(
                            fromByteCount: Int64(available), countStyle: .file),
                        "required": ByteCountFormatter.string(
                            fromByteCount: Int64(requiredSpace), countStyle: .file),
                    ]
                )
            } else {
                // Prioritize SPARSE format for better sparse file handling
                Logger.info("Attempting conversion to SPARSE format...")
                let process = Process()
                process.executableURL = URL(fileURLWithPath: "/usr/bin/hdiutil")
                process.arguments = [
                    "convert",
                    outputURL.path,  // Source: our assembled file
                    "-format", "SPARSE",  // Format: SPARSE (best for sparse images)
                    "-o", outputURL.path,  // Output: overwrite with converted image
                ]

                let errorPipe = Pipe()
                process.standardError = errorPipe
                process.standardOutput = errorPipe

                try process.run()
                process.waitUntilExit()

                // Check for errors
                let outputData = errorPipe.fileHandleForReading.readDataToEndOfFile()
                if !outputData.isEmpty, let outputString = String(data: outputData, encoding: .utf8)
                {
                    Logger.info("hdiutil output: \(outputString)")
                }

                if process.terminationStatus == 0 {
                    // Find the potentially renamed formatted file
                    let formattedFile = findFormattedFile(tempFormatted: outputURL) ?? outputURL
                    // If the output path is different, remove the original and move the new one
                    if formattedFile.path != outputURL.path {
                        try? FileManager.default.removeItem(at: outputURL)
                        try FileManager.default.moveItem(at: formattedFile, to: outputURL)
                    }
                    Logger.info("Successfully converted disk image to proper format (SPARSE)")
                } else {
                    Logger.error(
                        "Failed to convert disk image to SPARSE format. VM might not start properly."
                    )
                    // If SPARSE failed, maybe try UDRW as a last resort?
                    // For now, we'll just log the error.
                }
            }

            Logger.info("Disk image reassembly completed")
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
        progress: isolated ProgressTracker,
        manifestId: String? = nil
    ) async throws {
        var lastError: Error?

        // Create a shared session configuration for all download attempts
        let config = URLSessionConfiguration.default
        config.timeoutIntervalForRequest = 60
        config.timeoutIntervalForResource = 3600
        config.waitsForConnectivity = true
        config.httpMaximumConnectionsPerHost = 6
        config.httpShouldUsePipelining = true
        config.requestCachePolicy = .reloadIgnoringLocalCacheData

        // Enable HTTP/2 when available
        if #available(macOS 13.0, *) {
            config.httpAdditionalHeaders = ["Connection": "keep-alive"]
        }

        // Check for TCP window size and optimize if possible
        if getTCPReceiveWindowSize() != nil {
            config.networkServiceType = .responsiveData
        }

        // Create one session to be reused across retries
        let session = URLSession(configuration: config)

        for attempt in 1...maxRetries {
            do {
                var request = URLRequest(
                    url: URL(string: "https://\(self.registry)/v2/\(repository)/blobs/\(digest)")!)
                request.addValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
                request.addValue(mediaType, forHTTPHeaderField: "Accept")
                request.timeoutInterval = 60

                // Add Accept-Encoding for compressed transfer if content isn't already compressed
                if !mediaType.contains("gzip") && !mediaType.contains("compressed") {
                    request.addValue("gzip, deflate", forHTTPHeaderField: "Accept-Encoding")
                }

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

                // Cache the downloaded layer if caching is enabled
                if cachingEnabled, let manifestId = manifestId {
                    let cachedLayer = getCachedLayerPath(manifestId: manifestId, digest: digest)
                    if FileManager.default.fileExists(atPath: cachedLayer.path) {
                        try FileManager.default.removeItem(at: cachedLayer)
                    }
                    try FileManager.default.copyItem(at: url, to: cachedLayer)
                }

                // Mark download as complete regardless of caching
                markDownloadComplete(digest)
                return

            } catch {
                lastError = error
                if attempt < maxRetries {
                    // Exponential backoff with jitter for retries
                    let baseDelay = Double(attempt) * 2
                    let jitter = Double.random(in: 0...1)
                    let delay = baseDelay + jitter
                    try await Task.sleep(nanoseconds: UInt64(delay * 1_000_000_000))

                    Logger.info("Retrying download (attempt \(attempt+1)/\(maxRetries)): \(digest)")
                }
            }
        }

        throw lastError ?? PullError.layerDownloadFailed(digest)
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

        // Define chunk size parameters
        let safeMinimumChunkSize = 128 * 1024  // Reduced minimum for constrained systems
        let defaultChunkSize = 512 * 1024  // Standard default / minimum for non-constrained
        let constrainedCap = 512 * 1024  // Lower cap for constrained systems
        let standardCap = 2 * 1024 * 1024  // Standard cap for non-constrained systems

        // If we can't get memory info, return a reasonable default
        guard result == KERN_SUCCESS else {
            Logger.info(
                "Could not get VM statistics, using default chunk size: \(defaultChunkSize) bytes")
            return defaultChunkSize
        }

        // Calculate free memory in bytes
        let pageSize = 4096  // Use a constant page size assumption
        let freeMemory = UInt64(stats.free_count) * UInt64(pageSize)
        let isConstrained = determineIfMemoryConstrained()  // Check if generally constrained

        // Extremely constrained (< 512MB free) -> use absolute minimum
        if freeMemory < 536_870_912 {  // 512MB
            Logger.info(
                "System extremely memory constrained (<512MB free), using minimum chunk size: \(safeMinimumChunkSize) bytes"
            )
            return safeMinimumChunkSize
        }

        // Generally constrained -> use adaptive size with lower cap
        if isConstrained {
            let adaptiveSize = min(
                max(Int(freeMemory / 1000), safeMinimumChunkSize), constrainedCap)
            Logger.info(
                "System memory constrained, using adaptive chunk size capped at \(constrainedCap) bytes: \(adaptiveSize) bytes"
            )
            return adaptiveSize
        }

        // Not constrained -> use original adaptive logic with standard cap
        let adaptiveSize = min(max(Int(freeMemory / 1000), defaultChunkSize), standardCap)
        Logger.info(
            "System has sufficient memory, using adaptive chunk size capped at \(standardCap) bytes: \(adaptiveSize) bytes"
        )
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

    // Helper method to determine network quality
    private func determineNetworkQuality() -> Int {
        // Default quality is medium (3)
        var quality = 3

        // A simple ping test to determine network quality
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/sbin/ping")
        process.arguments = ["-c", "3", "-q", self.registry]

        let outputPipe = Pipe()
        process.standardOutput = outputPipe
        process.standardError = outputPipe

        do {
            try process.run()
            process.waitUntilExit()

            let outputData = try outputPipe.fileHandleForReading.readToEnd() ?? Data()
            if let output = String(data: outputData, encoding: .utf8) {
                // Check for average ping time
                if let avgTimeRange = output.range(
                    of: "= [0-9.]+/([0-9.]+)/", options: .regularExpression)
                {
                    let avgSubstring = output[avgTimeRange]
                    if let avgString = avgSubstring.split(separator: "/").dropFirst().first,
                        let avgTime = Double(avgString)
                    {

                        // Classify network quality based on ping time
                        if avgTime < 50 {
                            quality = 5  // Excellent
                        } else if avgTime < 100 {
                            quality = 4  // Good
                        } else if avgTime < 200 {
                            quality = 3  // Average
                        } else if avgTime < 300 {
                            quality = 2  // Poor
                        } else {
                            quality = 1  // Very poor
                        }
                    }
                }
            }
        } catch {
            // Default to medium if ping fails
            Logger.info("Failed to determine network quality, using default settings")
        }

        return quality
    }

    // Helper method to calculate optimal concurrency based on system capabilities
    private func calculateOptimalConcurrency(memoryConstrained: Bool, networkQuality: Int) -> Int {
        // Base concurrency based on network quality (1-5)
        let baseThreads = min(networkQuality * 2, 8)

        if memoryConstrained {
            // Reduce concurrency for memory-constrained systems
            return max(2, baseThreads / 2)
        }

        // Physical cores available on the system
        let cores = ProcessInfo.processInfo.processorCount

        // Adaptive approach: 1-2 threads per core depending on network quality
        let threadsPerCore = (networkQuality >= 4) ? 2 : 1
        let systemBasedThreads = min(cores * threadsPerCore, 12)

        // Take the larger of network-based and system-based concurrency
        return max(baseThreads, systemBasedThreads)
    }

    // Helper to get optimal TCP window size
    private func getTCPReceiveWindowSize() -> Int? {
        // Try to query system TCP window size
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/sbin/sysctl")
        process.arguments = ["net.inet.tcp.recvspace"]

        let outputPipe = Pipe()
        process.standardOutput = outputPipe

        do {
            try process.run()
            process.waitUntilExit()

            let outputData = try outputPipe.fileHandleForReading.readToEnd() ?? Data()
            if let output = String(data: outputData, encoding: .utf8),
                let valueStr = output.split(separator: ":").last?.trimmingCharacters(
                    in: .whitespacesAndNewlines),
                let value = Int(valueStr)
            {
                return value
            }
        } catch {
            // Ignore errors, we'll use defaults
        }

        return nil
    }

    // Add helper to check media type and get decompress command
    private func getDecompressionCommand(for mediaType: String) -> String? {
        // Determine appropriate decompression command based on layer media type
        Logger.info("Determining decompression command for media type: \(mediaType)")

        // For the specific format that appears in our GHCR repository, skip decompression attempts
        // These files are labeled +lzfse but aren't actually in Apple Archive format
        if mediaType.contains("+lzfse;part.number=") {
            Logger.info("Detected LZFSE part file, using direct copy instead of decompression")
            return nil
        }

        // Check for LZFSE or Apple Archive format anywhere in the media type string
        // The format may include part information like: application/octet-stream+lzfse;part.number=1;part.total=38
        if mediaType.contains("+lzfse") || mediaType.contains("+aa") {
            // Apple Archive format requires special handling
            if let aaPath = findExecutablePath(for: "aa") {
                Logger.info("Found Apple Archive tool at: \(aaPath)")
                return "apple_archive:\(aaPath)"
            } else {
                Logger.error(
                    "Apple Archive tool (aa) not found in PATH, falling back to default path")

                // Check if the default path exists
                let defaultPath = "/usr/bin/aa"
                if FileManager.default.isExecutableFile(atPath: defaultPath) {
                    Logger.info("Default Apple Archive tool exists at: \(defaultPath)")
                } else {
                    Logger.error("Default Apple Archive tool not found at: \(defaultPath)")
                }

                return "apple_archive:/usr/bin/aa"
            }
        } else {
            Logger.info(
                "Unsupported media type: \(mediaType) - only Apple Archive (+lzfse/+aa) is supported"
            )
            return nil
        }
    }

    // Helper to find executables (optional, or hardcode paths)
    private func findExecutablePath(for executableName: String) -> String? {
        let pathEnv =
            ProcessInfo.processInfo.environment["PATH"]
            ?? "/usr/bin:/bin:/usr/sbin:/sbin:/usr/local/bin:/opt/homebrew/bin"
        let paths = pathEnv.split(separator: ":")
        for path in paths {
            let executablePath = URL(fileURLWithPath: String(path)).appendingPathComponent(
                executableName
            ).path
            if FileManager.default.isExecutableFile(atPath: executablePath) {
                return executablePath
            }
        }
        return nil
    }

    // Helper function to extract uncompressed disk size from config.json
    private func getUncompressedSizeFromConfig(configPath: URL) -> UInt64? {
        guard FileManager.default.fileExists(atPath: configPath.path) else {
            Logger.info("Config file not found: \(configPath.path)")
            return nil
        }

        do {
            let configData = try Data(contentsOf: configPath)
            let decoder = JSONDecoder()
            let ociConfig = try decoder.decode(OCIConfig.self, from: configData)

            if let sizeString = ociConfig.annotations?.uncompressedSize,
                let size = UInt64(sizeString)
            {
                Logger.info("Found uncompressed disk size annotation: \(size) bytes")
                return size
            } else {
                Logger.info("No uncompressed disk size annotation found in config.json")
                return nil
            }
        } catch {
            Logger.error("Failed to parse config.json for uncompressed size: \(error)")
            return nil
        }
    }

    // Helper function to find formatted file with potential extensions
    private func findFormattedFile(tempFormatted: URL) -> URL? {
        // Check for the exact path first
        if FileManager.default.fileExists(atPath: tempFormatted.path) {
            return tempFormatted
        }

        // Check with .dmg extension
        let dmgPath = tempFormatted.path + ".dmg"
        if FileManager.default.fileExists(atPath: dmgPath) {
            return URL(fileURLWithPath: dmgPath)
        }

        // Check with .sparseimage extension
        let sparsePath = tempFormatted.path + ".sparseimage"
        if FileManager.default.fileExists(atPath: sparsePath) {
            return URL(fileURLWithPath: sparsePath)
        }

        // Try to find any file with the same basename
        do {
            let files = try FileManager.default.contentsOfDirectory(
                at: tempFormatted.deletingLastPathComponent(),
                includingPropertiesForKeys: nil)
            if let matchingFile = files.first(where: {
                $0.lastPathComponent.starts(with: tempFormatted.lastPathComponent)
            }) {
                return matchingFile
            }
        } catch {
            Logger.error("Failed to list directory contents: \(error)")
        }

        return nil
    }

    // Helper function to decompress LZFSE compressed disk image
    @discardableResult
    private func decompressLZFSEImage(inputPath: String, outputPath: String? = nil) -> Bool {
        Logger.info("Attempting to decompress LZFSE compressed disk image using sparse pipe...")

        let finalOutputPath = outputPath ?? inputPath  // If outputPath is nil, we'll overwrite input
        let tempFinalPath = finalOutputPath + ".ddsparse.tmp"  // Temporary name during dd operation

        // Ensure the temporary file doesn't exist from a previous failed run
        try? FileManager.default.removeItem(atPath: tempFinalPath)

        // Process 1: compression_tool
        let process1 = Process()
        process1.executableURL = URL(fileURLWithPath: "/usr/bin/compression_tool")
        process1.arguments = [
            "-decode",
            "-i", inputPath,
            "-o", "/dev/stdout",  // Write to standard output
        ]

        // Process 2: dd
        let process2 = Process()
        process2.executableURL = URL(fileURLWithPath: "/bin/dd")
        process2.arguments = [
            "if=/dev/stdin",  // Read from standard input
            "of=\(tempFinalPath)",  // Write to the temporary final path
            "conv=sparse",  // Use sparse conversion
            "bs=1m",  // Use a reasonable block size (e.g., 1MB)
        ]

        // Create pipes
        let pipe = Pipe()  // Connects process1 stdout to process2 stdin
        let errorPipe1 = Pipe()
        let errorPipe2 = Pipe()

        process1.standardOutput = pipe
        process1.standardError = errorPipe1

        process2.standardInput = pipe
        process2.standardError = errorPipe2

        do {
            Logger.info("Starting decompression pipe: compression_tool | dd conv=sparse...")
            // Start processes
            try process1.run()
            try process2.run()

            // Close the write end of the pipe for process2 to prevent hanging
            // This might not be strictly necessary if process1 exits cleanly, but safer.
            // Note: Accessing fileHandleForWriting after run can be tricky.
            // We rely on process1 exiting to signal EOF to process2.

            process1.waitUntilExit()
            process2.waitUntilExit()  // Wait for dd to finish processing the stream

            // --- Check for errors ---
            let errorData1 = errorPipe1.fileHandleForReading.readDataToEndOfFile()
            if !errorData1.isEmpty,
                let errorString = String(data: errorData1, encoding: .utf8)?.trimmingCharacters(
                    in: .whitespacesAndNewlines), !errorString.isEmpty
            {
                Logger.error("compression_tool stderr: \(errorString)")
            }
            let errorData2 = errorPipe2.fileHandleForReading.readDataToEndOfFile()
            if !errorData2.isEmpty,
                let errorString = String(data: errorData2, encoding: .utf8)?.trimmingCharacters(
                    in: .whitespacesAndNewlines), !errorString.isEmpty
            {
                // dd often reports blocks in/out to stderr, filter that if needed, but log for now
                Logger.info("dd stderr: \(errorString)")
            }

            // Check termination statuses
            let status1 = process1.terminationStatus
            let status2 = process2.terminationStatus

            if status1 != 0 || status2 != 0 {
                Logger.error(
                    "Pipe command failed. compression_tool status: \(status1), dd status: \(status2)"
                )
                try? FileManager.default.removeItem(atPath: tempFinalPath)  // Clean up failed attempt
                return false
            }

            // --- Validation ---
            if FileManager.default.fileExists(atPath: tempFinalPath) {
                let fileSize =
                    (try? FileManager.default.attributesOfItem(atPath: tempFinalPath)[.size]
                        as? UInt64) ?? 0
                let actualUsage = getActualDiskUsage(path: tempFinalPath)
                Logger.info(
                    "Piped decompression successful - Allocated: \(ByteCountFormatter.string(fromByteCount: Int64(fileSize), countStyle: .file)), Actual Usage: \(ByteCountFormatter.string(fromByteCount: Int64(actualUsage), countStyle: .file))"
                )

                // Basic header validation
                var isValid = false
                if let fileHandle = FileHandle(forReadingAtPath: tempFinalPath) {
                    if let data = try? fileHandle.read(upToCount: 512), data.count >= 512,
                        data[510] == 0x55 && data[511] == 0xAA
                    {
                        isValid = true
                    }
                    // Ensure handle is closed regardless of validation outcome
                    try? fileHandle.close()
                } else {
                    Logger.error(
                        "Validation Error: Could not open decompressed file handle for reading.")
                }

                if isValid {
                    Logger.info("Decompressed file appears to be a valid disk image.")

                    // Move the final file into place
                    // If outputPath was nil, we need to replace the original inputPath
                    if outputPath == nil {
                        // Backup original only if it's different from the temp path
                        if inputPath != tempFinalPath {
                            try? FileManager.default.copyItem(
                                at: URL(fileURLWithPath: inputPath),
                                to: URL(fileURLWithPath: inputPath + ".compressed.bak"))
                            try? FileManager.default.removeItem(at: URL(fileURLWithPath: inputPath))
                        }
                        try FileManager.default.moveItem(
                            at: URL(fileURLWithPath: tempFinalPath),
                            to: URL(fileURLWithPath: inputPath))
                        Logger.info("Replaced original file with sparsely decompressed version.")
                    } else {
                        // If outputPath was specified, move it there (overwrite if needed)
                        try? FileManager.default.removeItem(
                            at: URL(fileURLWithPath: finalOutputPath))  // Remove existing if overwriting
                        try FileManager.default.moveItem(
                            at: URL(fileURLWithPath: tempFinalPath),
                            to: URL(fileURLWithPath: finalOutputPath))
                        Logger.info("Moved sparsely decompressed file to: \(finalOutputPath)")
                    }
                    return true
                } else {
                    Logger.error(
                        "Validation failed: Decompressed file header is invalid or file couldn't be read. Cleaning up."
                    )
                    try? FileManager.default.removeItem(atPath: tempFinalPath)
                    return false
                }
            } else {
                Logger.error(
                    "Piped decompression failed: Output file '\(tempFinalPath)' not found after dd completed."
                )
                return false
            }

        } catch {
            Logger.error("Error running decompression pipe command: \(error)")
            try? FileManager.default.removeItem(atPath: tempFinalPath)  // Clean up on error
            return false
        }
    }

    // Helper function to get actual disk usage of a file
    private func getActualDiskUsage(path: String) -> UInt64 {
        let task = Process()
        task.executableURL = URL(fileURLWithPath: "/usr/bin/du")
        task.arguments = ["-k", path]  // -k for 1024-byte blocks

        let pipe = Pipe()
        task.standardOutput = pipe

        do {
            try task.run()
            task.waitUntilExit()

            let data = pipe.fileHandleForReading.readDataToEndOfFile()
            if let output = String(data: data, encoding: .utf8),
                let size = UInt64(output.split(separator: "\t").first ?? "0")
            {
                return size * 1024  // Convert from KB to bytes
            }
        } catch {
            Logger.error("Failed to get actual disk usage: \(error)")
        }

        return 0
    }
}
