import ArgumentParser
import CommonCrypto
import Compression  // Add this import
import Darwin
import Foundation
import Swift

// Extension to calculate SHA256 hash
extension Data {
    func sha256String() -> String {
        let hash = self.withUnsafeBytes { (bytes: UnsafeRawBufferPointer) -> [UInt8] in
            var hash = [UInt8](repeating: 0, count: Int(CC_SHA256_DIGEST_LENGTH))
            CC_SHA256(bytes.baseAddress, CC_LONG(self.count), &hash)
            return hash
        }
        return hash.map { String(format: "%02x", $0) }.joined()
    }
}

// Push-related errors
enum PushError: Error {
    case uploadInitiationFailed
    case blobUploadFailed
    case manifestPushFailed
    case authenticationFailed
    case missingToken
    case invalidURL
    case lz4NotFound  // Added error case
    case invalidMediaType  // Added during part refactoring
    case missingUncompressedSizeAnnotation  // Added for sparse file handling
    case fileCreationFailed(String)  // Added for sparse file handling
    case reassemblySetupFailed(path: String, underlyingError: Error?)  // Added for sparse file handling
    case missingPart(Int)  // Added for sparse file handling
    case layerDownloadFailed(String)  // Added for download retries
    case manifestFetchFailed  // Added for manifest fetching
    case insufficientPermissions(String)  // Added for permission issues
}

// Define a specific error type for when no underlying error exists
struct NoSpecificUnderlyingError: Error, CustomStringConvertible {
    var description: String { "No specific underlying error was provided." }
}

struct ChunkMetadata: Codable {
    let uncompressedDigest: String
    let uncompressedSize: UInt64
    let compressedDigest: String
    let compressedSize: Int
}

// Define struct to decode relevant parts of config.json
struct OCIManifestLayer {
    let mediaType: String
    let size: Int
    let digest: String
    let uncompressedSize: UInt64?
    let uncompressedContentDigest: String?

    init(
        mediaType: String, size: Int, digest: String, uncompressedSize: UInt64? = nil,
        uncompressedContentDigest: String? = nil
    ) {
        self.mediaType = mediaType
        self.size = size
        self.digest = digest
        self.uncompressedSize = uncompressedSize
        self.uncompressedContentDigest = uncompressedContentDigest
    }
}

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

// Actor to safely collect disk part information from concurrent tasks
actor DiskPartsCollector {
    // Store tuples of (sequentialPartNum, url)
    private var diskParts: [(Int, URL)] = []
    // Restore internal counter
    private var partCounter = 0

    // Adds a part and returns its assigned sequential number
    func addPart(url: URL) -> Int {
        partCounter += 1  // Use counter logic
        let partNum = partCounter
        diskParts.append((partNum, url))  // Store sequential number
        return partNum  // Return assigned sequential number
    }

    // Sort by the sequential part number (index 0 of tuple)
    func getSortedParts() -> [(Int, URL)] {
        return diskParts.sorted { $0.0 < $1.0 }
    }

    // Restore getTotalParts
    func getTotalParts() -> Int {
        return partCounter
    }
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

// Renamed struct
struct UploadStats {
    let totalBytes: Int64
    let uploadedBytes: Int64  // Renamed
    let elapsedTime: TimeInterval
    let averageSpeed: Double
    let peakSpeed: Double

    func formattedSummary() -> String {
        let bytesStr = ByteCountFormatter.string(fromByteCount: uploadedBytes, countStyle: .file)
        let avgSpeedStr = formatSpeed(averageSpeed)
        let peakSpeedStr = formatSpeed(peakSpeed)
        let timeStr = formatTime(elapsedTime)
        return """
            Upload Statistics:
            - Total uploaded: \(bytesStr)
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
    private let downloadProgress = ProgressTracker()  // Renamed for clarity
    private let uploadProgress = UploadProgressTracker()  // Added upload tracker
    private let cacheDirectory: URL
    private let downloadLock = NSLock()
    private var activeDownloads: [String] = []
    private let cachingEnabled: Bool

    // Constants for zero-skipping write logic
    private static let holeGranularityBytes = 4 * 1024 * 1024  // 4MB block size for checking zeros
    private static let zeroChunk = Data(count: holeGranularityBytes)

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
    ) async throws -> VMDirectory {
        guard !image.isEmpty else {
            throw ValidationError("Image name cannot be empty")
        }

        let home = Home()

        // Use provided name or derive from image
        let vmName = name ?? image.split(separator: ":").first.map(String.init) ?? ""
        
        // Determine if locationName is a direct path or a named storage location
        let vmDir: VMDirectory
        if let locationName = locationName, locationName.contains("/") || locationName.contains("\\") {
            // Direct path
            vmDir = try home.getVMDirectoryFromPath(vmName, storagePath: locationName)
        } else {
            // Named storage or default location
            vmDir = try home.getVMDirectory(vmName, storage: locationName)
        }

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
        let token = try await getToken(
            repository: "\(self.organization)/\(imageName)", scopes: ["pull"])

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
            await downloadProgress.setTotal(totalSize, files: totalFiles)

            // Process layers with limited concurrency
            Logger.info("Processing Image layers")
            Logger.info(
                "This may take several minutes depending on the image size and your internet connection. Please wait..."
            )

            // Add immediate progress indicator before starting downloads
            print(
                "[░░░░░░░░░░░░░░░░░░░░] 0% | Initializing downloads... | ETA: calculating...     ")
            fflush(stdout)

            // Instantiate the collector
            let diskPartsCollector = DiskPartsCollector()

            // Adaptive concurrency based on system capabilities
            let memoryConstrained = determineIfMemoryConstrained()
            let networkQuality = determineNetworkQuality()
            let maxConcurrentTasks = calculateOptimalConcurrency(
                memoryConstrained: memoryConstrained, networkQuality: networkQuality)

            Logger.info(
                "Using adaptive download configuration: Concurrency=\(maxConcurrentTasks), Memory-optimized=\(memoryConstrained)"
            )

            let counter = TaskCounter()
            var lz4LayerCount = 0  // Count lz4 layers found

            try await withThrowingTaskGroup(of: Int64.self) { group in
                for layer in manifest.layers {
                    if layer.mediaType == "application/vnd.oci.empty.v1+json" {
                        continue
                    }

                    while await counter.current() >= maxConcurrentTasks {
                        _ = try await group.next()
                        await counter.decrement()
                    }

                    // Identify disk parts by media type
                    if layer.mediaType == "application/octet-stream+lz4" {
                        // --- Handle LZ4 Disk Part Layer ---
                        lz4LayerCount += 1  // Increment count
                        let currentPartNum = lz4LayerCount  // Use the current count as the logical number for logging

                        let cachedLayer = getCachedLayerPath(
                            manifestId: manifestId, digest: layer.digest)
                        let digest = layer.digest
                        let size = layer.size

                        if memoryConstrained
                            && FileManager.default.fileExists(atPath: cachedLayer.path)
                        {
                            // Add to collector, get sequential number assigned by collector
                            let collectorPartNum = await diskPartsCollector.addPart(
                                url: cachedLayer)
                            // Log using the sequential number from collector for clarity if needed, or the lz4LayerCount
                            Logger.info(
                                "Using cached lz4 layer (part \(currentPartNum)) directly: \(cachedLayer.lastPathComponent) -> Collector #\(collectorPartNum)"
                            )
                            await downloadProgress.addProgress(Int64(size))
                            continue
                        } else {
                            // Download/Copy Path (Task Group)
                            group.addTask { [self] in
                                await counter.increment()
                                let finalPath: URL
                                if FileManager.default.fileExists(atPath: cachedLayer.path) {
                                    let tempPartURL = tempDownloadDir.appendingPathComponent(
                                        "disk.img.part.\(UUID().uuidString)")
                                    try FileManager.default.copyItem(
                                        at: cachedLayer, to: tempPartURL)
                                    await downloadProgress.addProgress(Int64(size))
                                    finalPath = tempPartURL
                                } else {
                                    let tempPartURL = tempDownloadDir.appendingPathComponent(
                                        "disk.img.part.\(UUID().uuidString)")
                                    if isDownloading(digest) {
                                        try await waitForExistingDownload(
                                            digest, cachedLayer: cachedLayer)
                                        if FileManager.default.fileExists(atPath: cachedLayer.path)
                                        {
                                            try FileManager.default.copyItem(
                                                at: cachedLayer, to: tempPartURL)
                                            await downloadProgress.addProgress(Int64(size))
                                            finalPath = tempPartURL
                                        } else {
                                            markDownloadStarted(digest)
                                            try await self.downloadLayer(
                                                repository: "\(self.organization)/\(imageName)",
                                                digest: digest, mediaType: layer.mediaType,
                                                token: token,
                                                to: tempPartURL, maxRetries: 5,
                                                progress: downloadProgress, manifestId: manifestId
                                            )
                                            finalPath = tempPartURL
                                        }
                                    } else {
                                        markDownloadStarted(digest)
                                        try await self.downloadLayer(
                                            repository: "\(self.organization)/\(imageName)",
                                            digest: digest, mediaType: layer.mediaType,
                                            token: token,
                                            to: tempPartURL, maxRetries: 5,
                                            progress: downloadProgress, manifestId: manifestId
                                        )
                                        finalPath = tempPartURL
                                    }
                                }
                                // Add to collector, get sequential number assigned by collector
                                let collectorPartNum = await diskPartsCollector.addPart(
                                    url: finalPath)
                                // Log using the sequential number from collector
                                Logger.info(
                                    "Assigned path for lz4 layer (part \(currentPartNum)): \(finalPath.lastPathComponent) -> Collector #\(collectorPartNum)"
                                )
                                await counter.decrement()
                                return Int64(size)
                            }
                        }
                    } else {
                        // --- Handle Non-Disk-Part Layer ---
                        let mediaType = layer.mediaType
                        let digest = layer.digest
                        let size = layer.size

                        // Determine output path based on media type
                        let outputURL: URL
                        switch mediaType {
                        case "application/vnd.oci.image.layer.v1.tar",
                            "application/octet-stream+gzip":  // Might be compressed disk.img single file?
                            outputURL = tempDownloadDir.appendingPathComponent("disk.img")
                        case "application/vnd.oci.image.config.v1+json":
                            outputURL = tempDownloadDir.appendingPathComponent("config.json")
                        case "application/octet-stream":  // Could be nvram or uncompressed single disk.img
                            // Heuristic: If a config.json already exists or is expected, assume this is nvram.
                            // This might need refinement if single disk images use octet-stream.
                            if manifest.config != nil {
                                outputURL = tempDownloadDir.appendingPathComponent("nvram.bin")
                            } else {
                                // Assume it's a single-file disk image if no config layer is present
                                outputURL = tempDownloadDir.appendingPathComponent("disk.img")
                            }
                        default:
                            Logger.info("Skipping unsupported layer media type: \(mediaType)")
                            continue  // Skip to the next layer
                        }

                        // Add task to download/copy the non-disk-part layer
                        group.addTask { [self] in
                            await counter.increment()
                            let cachedLayer = getCachedLayerPath(
                                manifestId: manifestId, digest: digest)

                            if FileManager.default.fileExists(atPath: cachedLayer.path) {
                                try FileManager.default.copyItem(at: cachedLayer, to: outputURL)
                                await downloadProgress.addProgress(Int64(size))
                            } else {
                                if isDownloading(digest) {
                                    try await waitForExistingDownload(
                                        digest, cachedLayer: cachedLayer)
                                    if FileManager.default.fileExists(atPath: cachedLayer.path) {
                                        try FileManager.default.copyItem(
                                            at: cachedLayer, to: outputURL)
                                        await downloadProgress.addProgress(Int64(size))
                                        await counter.decrement()  // Decrement before returning
                                        return Int64(size)
                                    }
                                }

                                markDownloadStarted(digest)
                                try await self.downloadLayer(
                                    repository: "\(self.organization)/\(imageName)",
                                    digest: digest, mediaType: mediaType, token: token,
                                    to: outputURL, maxRetries: 5,
                                    progress: downloadProgress, manifestId: manifestId
                                )
                                // Note: downloadLayer handles caching and marking download complete
                            }
                            await counter.decrement()
                            return Int64(size)
                        }
                    }
                }  // End for layer in manifest.layers

                // Wait for remaining tasks
                for try await _ in group {}
            }  // End TaskGroup

            // Display download statistics
            let stats = await downloadProgress.getDownloadStats()
            Logger.info("")  // New line after progress
            Logger.info(stats.formattedSummary())

            // Now that we've downloaded everything to the cache, use copyFromCache to create final VM files
            if cachingEnabled {
                Logger.info("Using copyFromCache method to properly preserve partition tables")
                try await copyFromCache(manifest: manifest, manifestId: manifestId, to: tempVMDir)
            } else {
                // Even if caching is disabled, we need to use copyFromCache to assemble the disk image
                // correctly with partition tables, then we'll clean up the cache afterward
                Logger.info("Caching disabled - using temporary cache to assemble VM files")
                try await copyFromCache(manifest: manifest, manifestId: manifestId, to: tempVMDir)
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

        // If caching is disabled, clean up the cache entry
        if !cachingEnabled {
            Logger.info("Caching disabled - cleaning up temporary cache entry")
            try? cleanupCacheEntry(manifestId: manifestId)
        }

        Logger.info("Download complete: Files extracted to \(vmDir.dir.path)")
        Logger.info(
            "Note: Actual disk usage is significantly lower than reported size due to macOS sparse file system"
        )
        Logger.info(
            "Run 'lume run \(vmName)' to reduce the disk image file size by using macOS sparse file system"
        )
        return vmDir
    }

    // Helper function to clean up a specific cache entry
    private func cleanupCacheEntry(manifestId: String) throws {
        let cacheDir = getImageCacheDirectory(manifestId: manifestId)

        if FileManager.default.fileExists(atPath: cacheDir.path) {
            Logger.info("Removing cache entry for manifest ID: \(manifestId)")
            try FileManager.default.removeItem(at: cacheDir)
        }
    }

    // Shared function to handle disk image creation - can be used by both cache hit and cache miss paths
    private func createDiskImageFromSource(
        sourceURL: URL,  // Source data to decompress
        destinationURL: URL,  // Where to create the disk image
        diskSize: UInt64  // Total size for the sparse file
    ) throws {
        Logger.info("Creating sparse disk image...")

        // Create empty destination file
        if FileManager.default.fileExists(atPath: destinationURL.path) {
            try FileManager.default.removeItem(at: destinationURL)
        }
        guard FileManager.default.createFile(atPath: destinationURL.path, contents: nil) else {
            throw PullError.fileCreationFailed(destinationURL.path)
        }

        // Create sparse file
        let outputHandle = try FileHandle(forWritingTo: destinationURL)
        try outputHandle.truncate(atOffset: diskSize)

        // Write test patterns at beginning and end
        Logger.info("Writing test patterns to verify writability...")
        let testPattern = "LUME_TEST_PATTERN".data(using: .utf8)!
        try outputHandle.seek(toOffset: 0)
        try outputHandle.write(contentsOf: testPattern)
        try outputHandle.seek(toOffset: diskSize - UInt64(testPattern.count))
        try outputHandle.write(contentsOf: testPattern)
        try outputHandle.synchronize()

        // Decompress the source data at offset 0
        Logger.info("Decompressing source data...")
        let bytesWritten = try decompressChunkAndWriteSparse(
            inputPath: sourceURL.path,
            outputHandle: outputHandle,
            startOffset: 0
        )
        Logger.info(
            "Decompressed \(ByteCountFormatter.string(fromByteCount: Int64(bytesWritten), countStyle: .file)) of data"
        )

        // Ensure data is written and close handle
        try outputHandle.synchronize()
        try outputHandle.close()

        // Run sync to flush filesystem
        let syncProcess = Process()
        syncProcess.executableURL = URL(fileURLWithPath: "/bin/sync")
        try syncProcess.run()
        syncProcess.waitUntilExit()

        // Optimize with cp -c
        if FileManager.default.fileExists(atPath: "/bin/cp") {
            Logger.info("Optimizing sparse file representation...")
            let optimizedPath = destinationURL.path + ".optimized"

            let process = Process()
            process.executableURL = URL(fileURLWithPath: "/bin/cp")
            process.arguments = ["-c", destinationURL.path, optimizedPath]

            try process.run()
            process.waitUntilExit()

            if process.terminationStatus == 0 {
                // Get optimization results
                let optimizedSize =
                    (try? FileManager.default.attributesOfItem(atPath: optimizedPath)[.size]
                        as? UInt64) ?? 0
                let originalUsage = getActualDiskUsage(path: destinationURL.path)
                let optimizedUsage = getActualDiskUsage(path: optimizedPath)

                Logger.info(
                    "Sparse optimization results: Before: \(ByteCountFormatter.string(fromByteCount: Int64(originalUsage), countStyle: .file)) actual usage, After: \(ByteCountFormatter.string(fromByteCount: Int64(optimizedUsage), countStyle: .file)) actual usage (Apparent size: \(ByteCountFormatter.string(fromByteCount: Int64(optimizedSize), countStyle: .file)))"
                )

                // Replace original with optimized
                try FileManager.default.removeItem(at: destinationURL)
                try FileManager.default.moveItem(
                    at: URL(fileURLWithPath: optimizedPath), to: destinationURL)
                Logger.info("Replaced with optimized sparse version")
            } else {
                Logger.info("Sparse optimization failed, using original file")
                try? FileManager.default.removeItem(atPath: optimizedPath)
            }
        }

        // Set permissions to 0644
        let chmodProcess = Process()
        chmodProcess.executableURL = URL(fileURLWithPath: "/bin/chmod")
        chmodProcess.arguments = ["0644", destinationURL.path]
        try chmodProcess.run()
        chmodProcess.waitUntilExit()

        // Final sync
        let finalSyncProcess = Process()
        finalSyncProcess.executableURL = URL(fileURLWithPath: "/bin/sync")
        try finalSyncProcess.run()
        finalSyncProcess.waitUntilExit()
    }

    // Function to simulate cache pull behavior for freshly downloaded images
    private func simulateCachePull(tempVMDir: URL) throws {
        Logger.info("Simulating cache pull behavior for freshly downloaded image...")

        // Find disk.img in tempVMDir
        let diskImgPath = tempVMDir.appendingPathComponent("disk.img")
        guard FileManager.default.fileExists(atPath: diskImgPath.path) else {
            Logger.info("No disk.img found to simulate cache pull behavior")
            return
        }

        // Get file attributes and size
        let attributes = try FileManager.default.attributesOfItem(atPath: diskImgPath.path)
        guard let diskSize = attributes[.size] as? UInt64, diskSize > 0 else {
            Logger.error("Could not determine disk.img size for simulation")
            return
        }

        Logger.info("Creating true disk image clone with partition table preserved...")

        // Create backup of original file
        let backupPath = tempVMDir.appendingPathComponent("disk.img.original")
        try FileManager.default.moveItem(at: diskImgPath, to: backupPath)

        // Let's first check if the original image has a partition table
        Logger.info("Checking if source image has a partition table...")
        let checkProcess = Process()
        checkProcess.executableURL = URL(fileURLWithPath: "/usr/bin/hdiutil")
        checkProcess.arguments = ["imageinfo", backupPath.path]

        let checkPipe = Pipe()
        checkProcess.standardOutput = checkPipe

        try checkProcess.run()
        checkProcess.waitUntilExit()

        let checkData = checkPipe.fileHandleForReading.readDataToEndOfFile()
        let checkOutput = String(data: checkData, encoding: .utf8) ?? ""
        Logger.info("Source image info: \(checkOutput)")

        // Try different methods in sequence until one works
        var success = false

        // Method 1: Use hdiutil convert to convert the image while preserving all data
        if !success {
            Logger.info("Trying hdiutil convert...")
            let tempPath = tempVMDir.appendingPathComponent("disk.img.temp")

            let convertProcess = Process()
            convertProcess.executableURL = URL(fileURLWithPath: "/usr/bin/hdiutil")
            convertProcess.arguments = [
                "convert",
                backupPath.path,
                "-format", "UDRO",  // Read-only first to preserve partition table
                "-o", tempPath.path,
            ]

            let convertOutPipe = Pipe()
            let convertErrPipe = Pipe()
            convertProcess.standardOutput = convertOutPipe
            convertProcess.standardError = convertErrPipe

            do {
                try convertProcess.run()
                convertProcess.waitUntilExit()

                let errData = convertErrPipe.fileHandleForReading.readDataToEndOfFile()
                let errOutput = String(data: errData, encoding: .utf8) ?? ""

                if convertProcess.terminationStatus == 0 {
                    Logger.info("hdiutil convert succeeded. Converting to writable format...")
                    // Now convert to writable format
                    let convertBackProcess = Process()
                    convertBackProcess.executableURL = URL(fileURLWithPath: "/usr/bin/hdiutil")
                    convertBackProcess.arguments = [
                        "convert",
                        tempPath.path,
                        "-format", "UDRW",  // Read-write format
                        "-o", diskImgPath.path,
                    ]

                    try convertBackProcess.run()
                    convertBackProcess.waitUntilExit()

                    if convertBackProcess.terminationStatus == 0 {
                        Logger.info(
                            "Successfully converted to writable format with partition table")
                        success = true
                    } else {
                        Logger.error("hdiutil convert to writable format failed")
                    }

                    // Clean up temporary image
                    try? FileManager.default.removeItem(at: tempPath)
                } else {
                    Logger.error("hdiutil convert failed: \(errOutput)")
                }
            } catch {
                Logger.error("Error executing hdiutil convert: \(error)")
            }
        }

        // Method 2: Try direct raw copy method
        if !success {
            Logger.info("Trying direct raw copy with dd...")

            // Create empty file first
            FileManager.default.createFile(atPath: diskImgPath.path, contents: nil)

            let ddProcess = Process()
            ddProcess.executableURL = URL(fileURLWithPath: "/bin/dd")
            ddProcess.arguments = [
                "if=\(backupPath.path)",
                "of=\(diskImgPath.path)",
                "bs=1m",  // Large block size
                "count=81920",  // Ensure we copy everything (80GB+ should be sufficient)
            ]

            let ddErrPipe = Pipe()
            ddProcess.standardError = ddErrPipe

            do {
                try ddProcess.run()
                ddProcess.waitUntilExit()

                let errData = ddErrPipe.fileHandleForReading.readDataToEndOfFile()
                let errOutput = String(data: errData, encoding: .utf8) ?? ""

                if ddProcess.terminationStatus == 0 {
                    Logger.info("Raw dd copy completed: \(errOutput)")
                    success = true
                } else {
                    Logger.error("Raw dd copy failed: \(errOutput)")
                }
            } catch {
                Logger.error("Error executing dd: \(error)")
            }
        }

        // Method 3: Use a more complex approach with disk mounting
        if !success {
            Logger.info("Trying advanced disk attach/detach approach...")

            // Mount the source disk image
            let attachProcess = Process()
            attachProcess.executableURL = URL(fileURLWithPath: "/usr/bin/hdiutil")
            attachProcess.arguments = ["attach", backupPath.path, "-nomount"]

            let attachPipe = Pipe()
            attachProcess.standardOutput = attachPipe

            try attachProcess.run()
            attachProcess.waitUntilExit()

            let attachData = attachPipe.fileHandleForReading.readDataToEndOfFile()
            let attachOutput = String(data: attachData, encoding: .utf8) ?? ""

            // Extract the disk device from output (/dev/diskN)
            var diskDevice: String? = nil
            if let diskMatch = attachOutput.range(
                of: "/dev/disk[0-9]+", options: .regularExpression)
            {
                diskDevice = String(attachOutput[diskMatch])
            }

            if let device = diskDevice {
                Logger.info("Source disk attached at \(device)")

                // Create a bootable disk image clone
                let createProcess = Process()
                createProcess.executableURL = URL(fileURLWithPath: "/usr/sbin/asr")
                createProcess.arguments = [
                    "restore",
                    "--source", device,
                    "--target", diskImgPath.path,
                    "--erase",
                    "--noprompt",
                ]

                let createPipe = Pipe()
                createProcess.standardOutput = createPipe

                do {
                    try createProcess.run()
                    createProcess.waitUntilExit()

                    let createOutput =
                        String(
                            data: createPipe.fileHandleForReading.readDataToEndOfFile(),
                            encoding: .utf8) ?? ""
                    Logger.info("asr output: \(createOutput)")

                    if createProcess.terminationStatus == 0 {
                        Logger.info("Successfully created bootable disk image clone!")
                        success = true
                    } else {
                        Logger.error("Failed to create bootable disk image clone")
                    }
                } catch {
                    Logger.error("Error executing asr: \(error)")
                }

                // Always detach the disk when done
                let detachProcess = Process()
                detachProcess.executableURL = URL(fileURLWithPath: "/usr/bin/hdiutil")
                detachProcess.arguments = ["detach", device]
                try? detachProcess.run()
                detachProcess.waitUntilExit()
            } else {
                Logger.error("Failed to extract disk device from hdiutil attach output")
            }
        }

        // Fallback: If none of the methods worked, revert to our previous method just to ensure we have a usable image
        if !success {
            Logger.info("All specialized methods failed. Reverting to basic copy...")

            // If the disk image file exists (from a failed attempt), remove it
            if FileManager.default.fileExists(atPath: diskImgPath.path) {
                try FileManager.default.removeItem(at: diskImgPath)
            }

            // Attempt a basic file copy which will at least give us something to work with
            try FileManager.default.copyItem(at: backupPath, to: diskImgPath)
        }

        // Optimize sparseness if possible
        if FileManager.default.fileExists(atPath: "/bin/cp") {
            Logger.info("Optimizing sparse file representation...")
            let optimizedPath = diskImgPath.path + ".optimized"

            let process = Process()
            process.executableURL = URL(fileURLWithPath: "/bin/cp")
            process.arguments = ["-c", diskImgPath.path, optimizedPath]

            try process.run()
            process.waitUntilExit()

            if process.terminationStatus == 0 {
                let optimizedSize =
                    (try? FileManager.default.attributesOfItem(atPath: optimizedPath)[.size]
                        as? UInt64) ?? 0
                let originalUsage = getActualDiskUsage(path: diskImgPath.path)
                let optimizedUsage = getActualDiskUsage(path: optimizedPath)

                Logger.info(
                    "Sparse optimization results: Before: \(ByteCountFormatter.string(fromByteCount: Int64(originalUsage), countStyle: .file)) actual usage, After: \(ByteCountFormatter.string(fromByteCount: Int64(optimizedUsage), countStyle: .file)) actual usage (Apparent size: \(ByteCountFormatter.string(fromByteCount: Int64(optimizedSize), countStyle: .file)))"
                )

                // Replace with optimized version
                try FileManager.default.removeItem(at: diskImgPath)
                try FileManager.default.moveItem(
                    at: URL(fileURLWithPath: optimizedPath), to: diskImgPath)
                Logger.info("Replaced with optimized sparse version")
            } else {
                Logger.info("Sparse optimization failed, using original file")
                try? FileManager.default.removeItem(atPath: optimizedPath)
            }
        }

        // Set permissions to 0644
        let chmodProcess = Process()
        chmodProcess.executableURL = URL(fileURLWithPath: "/bin/chmod")
        chmodProcess.arguments = ["0644", diskImgPath.path]
        try chmodProcess.run()
        chmodProcess.waitUntilExit()

        // Final sync
        let finalSyncProcess = Process()
        finalSyncProcess.executableURL = URL(fileURLWithPath: "/bin/sync")
        try finalSyncProcess.run()
        finalSyncProcess.waitUntilExit()

        // Verify the final disk image
        Logger.info("Verifying final disk image partition information...")
        let verifyProcess = Process()
        verifyProcess.executableURL = URL(fileURLWithPath: "/usr/bin/hdiutil")
        verifyProcess.arguments = ["imageinfo", diskImgPath.path]

        let verifyOutputPipe = Pipe()
        verifyProcess.standardOutput = verifyOutputPipe

        try verifyProcess.run()
        verifyProcess.waitUntilExit()

        let verifyOutputData = verifyOutputPipe.fileHandleForReading.readDataToEndOfFile()
        let verifyOutput = String(data: verifyOutputData, encoding: .utf8) ?? ""
        Logger.info("Final disk image verification:\n\(verifyOutput)")

        // Clean up backup file
        try FileManager.default.removeItem(at: backupPath)

        Logger.info(
            "Cache pull simulation completed successfully with partition table preservation")
    }

    private func copyFromCache(manifest: Manifest, manifestId: String, to destination: URL)
        async throws
    {
        Logger.info("Copying from cache...")

        // Define output URL and expected size variable scope here
        let outputURL = destination.appendingPathComponent("disk.img")
        var expectedTotalSize: UInt64? = nil  // Use optional to handle missing config

        // Instantiate collector
        let diskPartsCollector = DiskPartsCollector()
        var lz4LayerCount = 0  // Count lz4 layers found

        // First identify disk parts and non-disk files
        for layer in manifest.layers {
            let cachedLayer = getCachedLayerPath(manifestId: manifestId, digest: layer.digest)

            // Identify disk parts simply by media type
            if layer.mediaType == "application/octet-stream+lz4" {
                lz4LayerCount += 1  // Increment count

                // When caching is disabled, the file might not exist with the cache path name
                // Check if the file exists before trying to use it
                if !FileManager.default.fileExists(atPath: cachedLayer.path) {
                    Logger.info("Layer file not found in cache: \(cachedLayer.path) - skipping")
                    continue
                }

                // Add to collector. It will assign the sequential part number.
                let collectorPartNum = await diskPartsCollector.addPart(url: cachedLayer)
                Logger.info(
                    "Adding cached lz4 layer (part \(lz4LayerCount)) -> Collector #\(collectorPartNum): \(cachedLayer.lastPathComponent)"
                )
            } else {
                // --- Handle Non-Disk-Part Layer (from cache) ---
                let fileName: String
                switch layer.mediaType {
                case "application/vnd.oci.image.config.v1+json":
                    fileName = "config.json"
                case "application/octet-stream":
                    // Assume nvram if config layer exists, otherwise assume single disk image
                    fileName = manifest.config != nil ? "nvram.bin" : "disk.img"
                case "application/vnd.oci.image.layer.v1.tar",
                    "application/octet-stream+gzip":
                    // Assume disk image for these types as well if encountered in cache scenario
                    fileName = "disk.img"
                default:
                    Logger.info("Skipping unsupported cached layer media type: \(layer.mediaType)")
                    continue
                }

                // When caching is disabled, the file might not exist with the cache path name
                if !FileManager.default.fileExists(atPath: cachedLayer.path) {
                    Logger.info(
                        "Non-disk layer file not found in cache: \(cachedLayer.path) - skipping")
                    continue
                }

                // Copy the non-disk file directly from cache to destination
                try FileManager.default.copyItem(
                    at: cachedLayer,
                    to: destination.appendingPathComponent(fileName)
                )
            }
        }

        // --- Safely retrieve parts AFTER loop ---
        let diskPartSources = await diskPartsCollector.getSortedParts()  // Sorted by assigned sequential number
        let totalParts = await diskPartsCollector.getTotalParts()  // Get total count from collector

        Logger.info("Found \(totalParts) lz4 disk parts in cache to reassemble.")
        // --- End retrieving parts ---

        // Reassemble disk parts if needed
        // Use the count from the collector
        if !diskPartSources.isEmpty {
            // Use totalParts from collector directly
            Logger.info(
                "Reassembling \(totalParts) disk image parts using sparse file technique...")

            // Get uncompressed size from cached config file (needs to be copied first)
            let configURL = destination.appendingPathComponent("config.json")
            // Parse config.json to get uncompressed size *before* reassembly
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

            // Determine the size to use for the sparse file
            // Use: annotation size > VM config diskSize > fallback (error)
            if let size = uncompressedSize {
                Logger.info("Using uncompressed size from annotation: \(size) bytes")
                expectedTotalSize = size
            } else if let size = vmConfigDiskSize {
                Logger.info("Using diskSize from VM config: \(size) bytes")
                expectedTotalSize = size
            } else {
                // If neither is found in cache scenario, throw error as we cannot determine the size
                Logger.error(
                    "Missing both uncompressed size annotation and VM config diskSize for cached multi-part image."
                        + " Cannot reassemble."
                )
                throw PullError.missingUncompressedSizeAnnotation
            }

            // Now that expectedTotalSize is guaranteed to be non-nil, proceed with setup
            guard let sizeForTruncate = expectedTotalSize else {
                // This should not happen due to the checks above, but safety first
                let nilError: Error? = nil
                // Use nil-coalescing to provide a default error, appeasing the compiler
                throw PullError.reassemblySetupFailed(
                    path: outputURL.path, underlyingError: nilError ?? NoSpecificUnderlyingError())
            }

            // If we have just one disk part, use the shared function
            if totalParts == 1 {
                // Single part - use shared function
                let sourceURL = diskPartSources[0].1  // Get the first source URL (index 1 of the tuple)
                try createDiskImageFromSource(
                    sourceURL: sourceURL,
                    destinationURL: outputURL,
                    diskSize: sizeForTruncate
                )
            } else {
                // Multiple parts - we need to reassemble
                // Wrap file handle setup and sparse file creation within this block
                let outputHandle: FileHandle
                do {
                    // Ensure parent directory exists
                    try FileManager.default.createDirectory(
                        at: outputURL.deletingLastPathComponent(), withIntermediateDirectories: true
                    )
                    // Explicitly create the file first, removing old one if needed
                    if FileManager.default.fileExists(atPath: outputURL.path) {
                        try FileManager.default.removeItem(at: outputURL)
                    }
                    guard FileManager.default.createFile(atPath: outputURL.path, contents: nil)
                    else {
                        throw PullError.fileCreationFailed(outputURL.path)
                    }
                    // Open handle for writing
                    outputHandle = try FileHandle(forWritingTo: outputURL)
                    // Set the file size (creates sparse file)
                    try outputHandle.truncate(atOffset: sizeForTruncate)
                    Logger.info(
                        "Sparse file initialized for cache reassembly with size: \(ByteCountFormatter.string(fromByteCount: Int64(sizeForTruncate), countStyle: .file))"
                    )
                } catch {
                    Logger.error(
                        "Failed during setup for cached disk image reassembly: \(error.localizedDescription)",
                        metadata: ["path": outputURL.path])
                    throw PullError.reassemblySetupFailed(
                        path: outputURL.path, underlyingError: error)
                }

                // Ensure handle is closed when exiting this scope
                defer { try? outputHandle.close() }

                var reassemblyProgressLogger = ProgressLogger(threshold: 0.05)
                var currentOffset: UInt64 = 0

                // Iterate from 1 up to the total number of parts found by the collector
                for collectorPartNum in 1...totalParts {
                    // Find the source URL from our collected parts using the sequential collectorPartNum
                    guard
                        let sourceInfo = diskPartSources.first(where: { $0.0 == collectorPartNum })
                    else {
                        Logger.error(
                            "Missing required cached part number \(collectorPartNum) in collected parts during reassembly."
                        )
                        throw PullError.missingPart(collectorPartNum)
                    }
                    let sourceURL = sourceInfo.1  // Get URL from tuple

                    // Log using the sequential collector part number
                    Logger.info(
                        "Decompressing part \(collectorPartNum) of \(totalParts) from cache: \(sourceURL.lastPathComponent) at offset \(currentOffset)..."
                    )

                    // Always use the correct sparse decompression function
                    let decompressedBytesWritten = try decompressChunkAndWriteSparse(
                        inputPath: sourceURL.path,
                        outputHandle: outputHandle,
                        startOffset: currentOffset
                    )
                    currentOffset += decompressedBytesWritten
                    // Update progress (using sizeForTruncate which should be available)
                    reassemblyProgressLogger.logProgress(
                        current: Double(currentOffset) / Double(sizeForTruncate),
                        context: "Reassembling Cache")

                    try outputHandle.synchronize()  // Explicitly synchronize after each chunk
                }

                // Finalize progress, close handle (done by defer)
                reassemblyProgressLogger.logProgress(current: 1.0, context: "Reassembly Complete")

                // Add test patterns at the beginning and end of the file
                Logger.info("Writing test patterns to sparse file to verify integrity...")
                let testPattern = "LUME_TEST_PATTERN".data(using: .utf8)!
                try outputHandle.seek(toOffset: 0)
                try outputHandle.write(contentsOf: testPattern)
                try outputHandle.seek(toOffset: sizeForTruncate - UInt64(testPattern.count))
                try outputHandle.write(contentsOf: testPattern)
                try outputHandle.synchronize()

                // Ensure handle is properly synchronized before closing
                try outputHandle.synchronize()

                // Close handle explicitly instead of relying on defer
                try outputHandle.close()

                // Verify final size
                let finalSize =
                    (try? FileManager.default.attributesOfItem(atPath: outputURL.path)[.size]
                        as? UInt64) ?? 0
                Logger.info(
                    "Final disk image size from cache (before sparse file optimization): \(ByteCountFormatter.string(fromByteCount: Int64(finalSize), countStyle: .file))"
                )

                // Use the calculated sizeForTruncate for comparison
                if finalSize != sizeForTruncate {
                    Logger.info(
                        "Warning: Final reported size (\(finalSize) bytes) differs from expected size (\(sizeForTruncate) bytes), but this doesn't affect functionality"
                    )
                }

                Logger.info("Disk image reassembly completed")

                // Optimize sparseness for cached reassembly if on macOS
                if FileManager.default.fileExists(atPath: "/bin/cp") {
                    Logger.info("Optimizing sparse file representation for cached reassembly...")
                    let optimizedPath = outputURL.path + ".optimized"

                    let process = Process()
                    process.executableURL = URL(fileURLWithPath: "/bin/cp")
                    process.arguments = ["-c", outputURL.path, optimizedPath]

                    do {
                        try process.run()
                        process.waitUntilExit()

                        if process.terminationStatus == 0 {
                            // Get size of optimized file
                            let optimizedSize =
                                (try? FileManager.default.attributesOfItem(atPath: optimizedPath)[
                                    .size] as? UInt64) ?? 0
                            let originalUsage = getActualDiskUsage(path: outputURL.path)
                            let optimizedUsage = getActualDiskUsage(path: optimizedPath)

                            Logger.info(
                                "Sparse optimization results for cache: Before: \(ByteCountFormatter.string(fromByteCount: Int64(originalUsage), countStyle: .file)) actual usage, After: \(ByteCountFormatter.string(fromByteCount: Int64(optimizedUsage), countStyle: .file)) actual usage (Apparent size: \(ByteCountFormatter.string(fromByteCount: Int64(optimizedSize), countStyle: .file)))"
                            )

                            // Replace the original with the optimized version
                            try FileManager.default.removeItem(at: outputURL)
                            try FileManager.default.moveItem(
                                at: URL(fileURLWithPath: optimizedPath), to: outputURL)
                            Logger.info("Replaced cached reassembly with optimized sparse version")
                        } else {
                            Logger.info("Sparse optimization failed for cache, using original file")
                            try? FileManager.default.removeItem(atPath: optimizedPath)
                        }
                    } catch {
                        Logger.info(
                            "Error during sparse optimization for cache: \(error.localizedDescription)"
                        )
                        try? FileManager.default.removeItem(atPath: optimizedPath)
                    }
                }

                // Set permissions to ensure consistency
                let chmodProcess = Process()
                chmodProcess.executableURL = URL(fileURLWithPath: "/bin/chmod")
                chmodProcess.arguments = ["0644", outputURL.path]
                try chmodProcess.run()
                chmodProcess.waitUntilExit()
            }
        }

        Logger.info("Cache copy complete")
    }

    private func getToken(
        repository: String, scopes: [String] = ["pull"], requireAllScopes: Bool = false
    ) async throws
        -> String
    {
        let encodedRepo =
            repository.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? repository

        // Build scope string from scopes array
        let scopeString = scopes.joined(separator: ",")

        Logger.info("Requesting token with scopes: \(scopeString) for repository: \(repository)")

        // Request both pull and push scope for uploads
        let url = URL(
            string:
                "https://\(self.registry)/token?scope=repository:\(encodedRepo):\(scopeString)&service=\(self.registry)"
        )!

        var request = URLRequest(url: url)
        request.httpMethod = "GET"  // Token endpoint uses GET
        request.setValue("application/json", forHTTPHeaderField: "Accept")

        // *** Add Basic Authentication Header if credentials exist ***
        let (username, password) = getCredentialsFromEnvironment()
        if let username = username, let password = password, !username.isEmpty, !password.isEmpty {
            let authString = "\(username):\(password)"
            if let authData = authString.data(using: .utf8) {
                let base64Auth = authData.base64EncodedString()
                request.setValue("Basic \(base64Auth)", forHTTPHeaderField: "Authorization")
                Logger.info("Adding Basic Authentication header to token request")
            } else {
                Logger.error("Failed to encode credentials for Basic Auth")
            }
        } else {
            Logger.info("No credentials found in environment for token request")
            // Allow anonymous request for pull scope, but push scope likely requires auth
        }
        // *** End Basic Auth addition ***

        let (data, response) = try await URLSession.shared.data(for: request)

        // Check response status code *before* parsing JSON
        guard let httpResponse = response as? HTTPURLResponse else {
            throw PushError.authenticationFailed  // Or a more generic network error
        }

        // Handle errors based on status code
        if httpResponse.statusCode != 200 {
            // Special handling for push operations that require all scopes
            if requireAllScopes
                && (httpResponse.statusCode == 401 || httpResponse.statusCode == 403)
            {
                // Try to parse the error message from the response
                let errorResponse = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
                let errors = errorResponse?["errors"] as? [[String: Any]]
                let errorMessage = errors?.first?["message"] as? String ?? "Permission denied"

                Logger.error("Push permission denied: \(errorMessage)")
                Logger.error(
                    "Your token does not have 'packages:write' permission to \(repository)")
                Logger.error("Make sure you have the appropriate access rights to the repository")

                // Check if this is an organization repository
                if repository.contains("/") {
                    let orgName = repository.split(separator: "/").first.map(String.init) ?? ""
                    if orgName != "" {
                        Logger.error("For organization repositories (\(orgName)), you must:")
                        Logger.error("1. Be a member of the organization with write access")
                        Logger.error("2. Have a token with 'write:packages' scope")
                        Logger.error(
                            "3. The organization must allow you to create/publish packages")
                    }
                }

                throw PushError.insufficientPermissions("Push permission denied: \(errorMessage)")
            }

            // If we get 403 and we're requesting both pull and push, retry with just pull
            // ONLY if requireAllScopes is false
            if httpResponse.statusCode == 403 && scopes.contains("push")
                && scopes.contains("pull") && !requireAllScopes
            {
                Logger.info("Permission denied for push scope, retrying with pull scope only")
                return try await getToken(repository: repository, scopes: ["pull"])
            }

            // For pull scope only, if authentication fails, assume this is a public image
            // and continue without a token (empty string)
            if scopes == ["pull"] {
                Logger.info(
                    "Authentication failed for pull scope, assuming public image and continuing without token"
                )
                return ""
            }

            // Handle other authentication failures
            let responseBody = String(data: data, encoding: .utf8) ?? "(Could not decode body)"
            Logger.error(
                "Token request failed with status code: \(httpResponse.statusCode). Response: \(responseBody)"
            )

            throw PushError.authenticationFailed
        }

        let jsonResponse = try JSONSerialization.jsonObject(with: data) as? [String: Any]
        guard
            let token = jsonResponse?["token"] as? String ?? jsonResponse?["access_token"]
                as? String
        else {
            Logger.error("Token not found in registry response")
            throw PushError.missingToken
        }

        return token
    }

    private func fetchManifest(repository: String, tag: String, token: String) async throws -> (
        Manifest, String
    ) {
        var request = URLRequest(
            url: URL(string: "https://\(self.registry)/v2/\(repository)/manifests/\(tag)")!)

        // Only add Authorization header if token is not empty
        if !token.isEmpty {
            request.addValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }

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

                // Only add Authorization header if token is not empty
                if !token.isEmpty {
                    request.addValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
                }

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

                // Always save a copy to the cache directory for use by copyFromCache,
                // even if caching is disabled
                if let manifestId = manifestId {
                    let cachedLayer = getCachedLayerPath(manifestId: manifestId, digest: digest)
                    // Make sure cache directory exists
                    try FileManager.default.createDirectory(
                        at: cachedLayer.deletingLastPathComponent(),
                        withIntermediateDirectories: true
                    )

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

    // Function removed as it's not applicable to the observed manifest format
    /*
    private func extractPartInfo(from mediaType: String) -> (partNum: Int, total: Int)? {
        let pattern = #"part\\.number=(\\d+);part\\.total=(\\d+)"#
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
    */

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

    // New push method
    public func push(
        vmDirPath: String,
        imageName: String,
        tags: [String],
        chunkSizeMb: Int = 512,
        verbose: Bool = false,
        dryRun: Bool = false,
        reassemble: Bool = false
    ) async throws {
        Logger.info(
            "Pushing VM to registry",
            metadata: [
                "vm_path": vmDirPath,
                "imageName": imageName,
                "tags": "\(tags.joined(separator: ", "))",  // Log all tags
                "registry": registry,
                "organization": organization,
                "chunk_size": "\(chunkSizeMb)MB",
                "dry_run": "\(dryRun)",
                "reassemble": "\(reassemble)",
            ])

        // Check for credentials if not in dry-run mode
        if !dryRun {
            let (username, token) = getCredentialsFromEnvironment()
            if username == nil || token == nil {
                Logger.error(
                    "Missing GitHub credentials. Please set GITHUB_USERNAME and GITHUB_TOKEN environment variables"
                )
                Logger.error(
                    "Your token must have 'packages:read' and 'packages:write' permissions")
                throw PushError.authenticationFailed
            }

            Logger.info("Using GitHub credentials from environment variables")
        }

        // Remove tag parsing here, imageName is now passed directly
        // let components = image.split(separator: ":") ...
        // let imageTag = String(tag)

        // Get authentication token only if not in dry-run mode
        var token: String = ""
        if !dryRun {
            Logger.info("Getting registry authentication token")
            token = try await getToken(
                repository: "\(self.organization)/\(imageName)",
                scopes: ["pull", "push"],
                requireAllScopes: true)  // Require push scope, don't fall back to pull-only
        } else {
            Logger.info("Dry run mode: skipping authentication token request")
        }

        // Create working directory inside the VM folder for caching/resuming
        let workDir = URL(fileURLWithPath: vmDirPath).appendingPathComponent(".lume_push_cache")
        try FileManager.default.createDirectory(at: workDir, withIntermediateDirectories: true)
        Logger.info("Using push cache directory: \(workDir.path)")

        // Get VM files that need to be pushed using vmDirPath
        let diskPath = URL(fileURLWithPath: vmDirPath).appendingPathComponent("disk.img")
        let configPath = URL(fileURLWithPath: vmDirPath).appendingPathComponent("config.json")
        let nvramPath = URL(fileURLWithPath: vmDirPath).appendingPathComponent("nvram.bin")

        var layers: [OCIManifestLayer] = []
        var uncompressedDiskSize: UInt64? = nil

        // Process config.json
        let cachedConfigPath = workDir.appendingPathComponent("config.json")
        var configDigest: String? = nil
        var configSize: Int? = nil

        if FileManager.default.fileExists(atPath: cachedConfigPath.path) {
            Logger.info("Using cached config.json")
            do {
                let configData = try Data(contentsOf: cachedConfigPath)
                configDigest = "sha256:" + configData.sha256String()
                configSize = configData.count
                // Try to get uncompressed disk size from cached config
                if let vmConfig = try? JSONDecoder().decode(VMConfig.self, from: configData) {
                    uncompressedDiskSize = vmConfig.diskSize
                    Logger.info(
                        "Found disk size in cached config: \(uncompressedDiskSize ?? 0) bytes")
                }
            } catch {
                Logger.error("Failed to read cached config.json: \(error). Will re-process.")
                // Force re-processing by leaving configDigest nil
            }
        } else if FileManager.default.fileExists(atPath: configPath.path) {
            Logger.info("Processing config.json")
            let configData = try Data(contentsOf: configPath)
            configDigest = "sha256:" + configData.sha256String()
            configSize = configData.count
            try configData.write(to: cachedConfigPath)  // Save to cache
            // Try to get uncompressed disk size from original config
            if let vmConfig = try? JSONDecoder().decode(VMConfig.self, from: configData) {
                uncompressedDiskSize = vmConfig.diskSize
                Logger.info("Found disk size in config: \(uncompressedDiskSize ?? 0) bytes")
            }
        }

        if var digest = configDigest, let size = configSize {  // Use 'var' to modify if uploaded
            if !dryRun {
                // Upload only if not in dry-run mode and blob doesn't exist
                if !(try await blobExists(
                    repository: "\(self.organization)/\(imageName)", digest: digest, token: token))
                {
                    Logger.info("Uploading config.json blob")
                    let configData = try Data(contentsOf: cachedConfigPath)  // Read from cache for upload
                    digest = try await uploadBlobFromData(
                        repository: "\(self.organization)/\(imageName)",
                        data: configData,
                        token: token
                    )
                } else {
                    Logger.info("Config blob already exists on registry")
                }
            }
            // Add config layer
            layers.append(
                OCIManifestLayer(
                    mediaType: "application/vnd.oci.image.config.v1+json",
                    size: size,
                    digest: digest
                ))
        }

        // Process nvram.bin
        let cachedNvramPath = workDir.appendingPathComponent("nvram.bin")
        var nvramDigest: String? = nil
        var nvramSize: Int? = nil

        if FileManager.default.fileExists(atPath: cachedNvramPath.path) {
            Logger.info("Using cached nvram.bin")
            do {
                let nvramData = try Data(contentsOf: cachedNvramPath)
                nvramDigest = "sha256:" + nvramData.sha256String()
                nvramSize = nvramData.count
            } catch {
                Logger.error("Failed to read cached nvram.bin: \(error). Will re-process.")
            }
        } else if FileManager.default.fileExists(atPath: nvramPath.path) {
            Logger.info("Processing nvram.bin")
            let nvramData = try Data(contentsOf: nvramPath)
            nvramDigest = "sha256:" + nvramData.sha256String()
            nvramSize = nvramData.count
            try nvramData.write(to: cachedNvramPath)  // Save to cache
        }

        if var digest = nvramDigest, let size = nvramSize {  // Use 'var'
            if !dryRun {
                // Upload only if not in dry-run mode and blob doesn't exist
                if !(try await blobExists(
                    repository: "\(self.organization)/\(imageName)", digest: digest, token: token))
                {
                    Logger.info("Uploading nvram.bin blob")
                    let nvramData = try Data(contentsOf: cachedNvramPath)  // Read from cache
                    digest = try await uploadBlobFromData(
                        repository: "\(self.organization)/\(imageName)",
                        data: nvramData,
                        token: token
                    )
                } else {
                    Logger.info("NVRAM blob already exists on registry")
                }
            }
            // Add nvram layer
            layers.append(
                OCIManifestLayer(
                    mediaType: "application/octet-stream",
                    size: size,
                    digest: digest
                ))
        }

        // Process disk.img
        if FileManager.default.fileExists(atPath: diskPath.path) {
            let diskAttributes = try FileManager.default.attributesOfItem(atPath: diskPath.path)
            let diskSize = diskAttributes[.size] as? UInt64 ?? 0
            let actualDiskSize = uncompressedDiskSize ?? diskSize
            Logger.info(
                "Processing disk.img in chunks",
                metadata: [
                    "disk_path": diskPath.path, "disk_size": "\(diskSize) bytes",
                    "actual_size": "\(actualDiskSize) bytes", "chunk_size": "\(chunkSizeMb)MB",
                ])
            let chunksDir = workDir.appendingPathComponent("disk.img.parts")
            try FileManager.default.createDirectory(
                at: chunksDir, withIntermediateDirectories: true)
            let chunkSizeBytes = chunkSizeMb * 1024 * 1024
            let totalChunks = Int((diskSize + UInt64(chunkSizeBytes) - 1) / UInt64(chunkSizeBytes))
            Logger.info("Splitting disk into \(totalChunks) chunks")
            let fileHandle = try FileHandle(forReadingFrom: diskPath)
            defer { try? fileHandle.close() }
            var pushedDiskLayers: [(index: Int, layer: OCIManifestLayer)] = []
            var diskChunks: [(index: Int, path: URL, digest: String)] = []

            try await withThrowingTaskGroup(of: (Int, OCIManifestLayer, URL, String).self) {
                group in
                let maxConcurrency = 4
                for chunkIndex in 0..<totalChunks {
                    if chunkIndex >= maxConcurrency {
                        if let res = try await group.next() {
                            pushedDiskLayers.append((res.0, res.1))
                            diskChunks.append((res.0, res.2, res.3))
                        }
                    }
                    group.addTask { [token, verbose, dryRun, organization, imageName] in
                        let chunkIndex = chunkIndex
                        let chunkPath = chunksDir.appendingPathComponent("chunk.\(chunkIndex)")
                        let metadataPath = chunksDir.appendingPathComponent(
                            "chunk_metadata.\(chunkIndex).json")
                        var layer: OCIManifestLayer? = nil
                        var finalCompressedDigest: String? = nil
                        if FileManager.default.fileExists(atPath: metadataPath.path),
                            FileManager.default.fileExists(atPath: chunkPath.path)
                        {
                            do {
                                let metadataData = try Data(contentsOf: metadataPath)
                                let metadata = try JSONDecoder().decode(
                                    ChunkMetadata.self, from: metadataData)
                                Logger.info(
                                    "Resuming chunk \(chunkIndex + 1)/\(totalChunks) from cache")
                                finalCompressedDigest = metadata.compressedDigest
                                if !dryRun {
                                    if !(try await self.blobExists(
                                        repository: "\(organization)/\(imageName)",
                                        digest: metadata.compressedDigest, token: token))
                                    {
                                        Logger.info("Uploading cached chunk \(chunkIndex + 1) blob")
                                        _ = try await self.uploadBlobFromPath(
                                            repository: "\(organization)/\(imageName)",
                                            path: chunkPath, digest: metadata.compressedDigest,
                                            token: token)
                                    } else {
                                        Logger.info(
                                            "Chunk \(chunkIndex + 1) blob already exists on registry"
                                        )
                                    }
                                }
                                layer = OCIManifestLayer(
                                    mediaType: "application/octet-stream+lz4",
                                    size: metadata.compressedSize,
                                    digest: metadata.compressedDigest,
                                    uncompressedSize: metadata.uncompressedSize,
                                    uncompressedContentDigest: metadata.uncompressedDigest)
                            } catch {
                                Logger.info(
                                    "Failed to load cached metadata/chunk for index \(chunkIndex): \(error). Re-processing."
                                )
                                finalCompressedDigest = nil
                                layer = nil
                            }
                        }
                        if layer == nil {
                            Logger.info("Processing chunk \(chunkIndex + 1)/\(totalChunks)")
                            let localFileHandle = try FileHandle(forReadingFrom: diskPath)
                            defer { try? localFileHandle.close() }
                            try localFileHandle.seek(toOffset: UInt64(chunkIndex * chunkSizeBytes))
                            let chunkData =
                                try localFileHandle.read(upToCount: chunkSizeBytes) ?? Data()
                            let uncompressedSize = UInt64(chunkData.count)
                            let uncompressedDigest = "sha256:" + chunkData.sha256String()
                            let compressedData =
                                try (chunkData as NSData).compressed(using: .lz4) as Data
                            let compressedSize = compressedData.count
                            let compressedDigest = "sha256:" + compressedData.sha256String()
                            try compressedData.write(to: chunkPath)
                            let metadata = ChunkMetadata(
                                uncompressedDigest: uncompressedDigest,
                                uncompressedSize: uncompressedSize,
                                compressedDigest: compressedDigest, compressedSize: compressedSize)
                            let metadataData = try JSONEncoder().encode(metadata)
                            try metadataData.write(to: metadataPath)
                            finalCompressedDigest = compressedDigest
                            if !dryRun {
                                if !(try await self.blobExists(
                                    repository: "\(organization)/\(imageName)",
                                    digest: compressedDigest, token: token))
                                {
                                    Logger.info("Uploading processed chunk \(chunkIndex + 1) blob")
                                    _ = try await self.uploadBlobFromPath(
                                        repository: "\(organization)/\(imageName)", path: chunkPath,
                                        digest: compressedDigest, token: token)
                                } else {
                                    Logger.info(
                                        "Chunk \(chunkIndex + 1) blob already exists on registry (processed fresh)"
                                    )
                                }
                            }
                            layer = OCIManifestLayer(
                                mediaType: "application/octet-stream+lz4", size: compressedSize,
                                digest: compressedDigest, uncompressedSize: uncompressedSize,
                                uncompressedContentDigest: uncompressedDigest)
                        }
                        guard let finalLayer = layer, let finalDigest = finalCompressedDigest else {
                            throw PushError.blobUploadFailed
                        }
                        if verbose {
                            Logger.info("Finished chunk \(chunkIndex + 1)/\(totalChunks)")
                        }
                        return (chunkIndex, finalLayer, chunkPath, finalDigest)
                    }
                }
                for try await (index, layer, path, digest) in group {
                    pushedDiskLayers.append((index, layer))
                    diskChunks.append((index, path, digest))
                }
            }
            layers.append(
                contentsOf: pushedDiskLayers.sorted { $0.index < $1.index }.map { $0.layer })
            diskChunks.sort { $0.index < $1.index }
            Logger.info("All disk chunks processed successfully")

            // --- Calculate Total Upload Size & Initialize Tracker ---
            if !dryRun {
                var totalUploadSizeBytes: Int64 = 0
                var totalUploadFiles: Int = 0
                // Add config size if it exists
                if let size = configSize {
                    totalUploadSizeBytes += Int64(size)
                    totalUploadFiles += 1
                }
                // Add nvram size if it exists
                if let size = nvramSize {
                    totalUploadSizeBytes += Int64(size)
                    totalUploadFiles += 1
                }
                // Add sizes of all compressed disk chunks
                let allChunkSizes = diskChunks.compactMap {
                    try? FileManager.default.attributesOfItem(atPath: $0.path.path)[.size] as? Int64
                        ?? 0
                }
                totalUploadSizeBytes += allChunkSizes.reduce(0, +)
                totalUploadFiles += totalChunks  // Use totalChunks calculated earlier

                if totalUploadSizeBytes > 0 {
                    Logger.info(
                        "Initializing upload progress: \(totalUploadFiles) files, total size: \(ByteCountFormatter.string(fromByteCount: totalUploadSizeBytes, countStyle: .file))"
                    )
                    await uploadProgress.setTotal(totalUploadSizeBytes, files: totalUploadFiles)
                    // Print initial progress bar
                    print(
                        "[░░░░░░░░░░░░░░░░░░░░] 0% (0/\(totalUploadFiles)) | Initializing upload... | ETA: calculating...     "
                    )
                    fflush(stdout)
                } else {
                    Logger.info("No files marked for upload.")
                }
            }
            // --- End Size Calculation & Init ---

            // Perform reassembly verification if requested in dry-run mode
            if dryRun && reassemble {
                Logger.info("=== REASSEMBLY MODE ===")
                Logger.info("Reassembling chunks to verify integrity...")
                let reassemblyDir = workDir.appendingPathComponent("reassembly")
                try FileManager.default.createDirectory(
                    at: reassemblyDir, withIntermediateDirectories: true)
                let reassembledFile = reassemblyDir.appendingPathComponent("reassembled_disk.img")

                // Pre-allocate a sparse file with the correct size
                Logger.info(
                    "Pre-allocating sparse file of \(ByteCountFormatter.string(fromByteCount: Int64(actualDiskSize), countStyle: .file))..."
                )
                if FileManager.default.fileExists(atPath: reassembledFile.path) {
                    try FileManager.default.removeItem(at: reassembledFile)
                }
                guard FileManager.default.createFile(atPath: reassembledFile.path, contents: nil)
                else {
                    throw PushError.fileCreationFailed(reassembledFile.path)
                }

                let outputHandle = try FileHandle(forWritingTo: reassembledFile)
                defer { try? outputHandle.close() }

                // Set the file size without writing data (creates a sparse file)
                try outputHandle.truncate(atOffset: actualDiskSize)

                // Add test patterns at start and end to verify writability
                let testPattern = "LUME_TEST_PATTERN".data(using: .utf8)!
                try outputHandle.seek(toOffset: 0)
                try outputHandle.write(contentsOf: testPattern)
                try outputHandle.seek(toOffset: actualDiskSize - UInt64(testPattern.count))
                try outputHandle.write(contentsOf: testPattern)
                try outputHandle.synchronize()

                Logger.info("Test patterns written to sparse file. File is ready for writing.")

                // Track reassembly progress
                var reassemblyProgressLogger = ProgressLogger(threshold: 0.05)
                var currentOffset: UInt64 = 0

                // Process each chunk in order
                for (index, cachedChunkPath, _) in diskChunks.sorted(by: { $0.index < $1.index }) {
                    Logger.info(
                        "Decompressing & writing part \(index + 1)/\(diskChunks.count): \(cachedChunkPath.lastPathComponent) at offset \(currentOffset)..."
                    )

                    // Always seek to the correct position
                    try outputHandle.seek(toOffset: currentOffset)

                    // Decompress and write the chunk
                    let decompressedBytesWritten = try decompressChunkAndWriteSparse(
                        inputPath: cachedChunkPath.path,
                        outputHandle: outputHandle,
                        startOffset: currentOffset
                    )

                    currentOffset += decompressedBytesWritten
                    reassemblyProgressLogger.logProgress(
                        current: Double(currentOffset) / Double(actualDiskSize),
                        context: "Reassembling"
                    )

                    // Ensure data is written before processing next part
                    try outputHandle.synchronize()
                }

                // Finalize progress
                reassemblyProgressLogger.logProgress(current: 1.0, context: "Reassembly Complete")
                Logger.info("")  // Newline

                // Close handle before post-processing
                try outputHandle.close()

                // Optimize sparseness if on macOS
                let optimizedFile = reassemblyDir.appendingPathComponent("optimized_disk.img")
                if FileManager.default.fileExists(atPath: "/bin/cp") {
                    Logger.info("Optimizing sparse file representation...")

                    let process = Process()
                    process.executableURL = URL(fileURLWithPath: "/bin/cp")
                    process.arguments = ["-c", reassembledFile.path, optimizedFile.path]

                    do {
                        try process.run()
                        process.waitUntilExit()

                        if process.terminationStatus == 0 {
                            // Get sizes of original and optimized files
                            let optimizedSize =
                                (try? FileManager.default.attributesOfItem(
                                    atPath: optimizedFile.path)[.size] as? UInt64) ?? 0
                            let originalUsage = getActualDiskUsage(path: reassembledFile.path)
                            let optimizedUsage = getActualDiskUsage(path: optimizedFile.path)

                            Logger.info(
                                "Sparse optimization results: Before: \(ByteCountFormatter.string(fromByteCount: Int64(originalUsage), countStyle: .file)) actual usage, After: \(ByteCountFormatter.string(fromByteCount: Int64(optimizedUsage), countStyle: .file)) actual usage (Apparent size: \(ByteCountFormatter.string(fromByteCount: Int64(optimizedSize), countStyle: .file)))"
                            )

                            // Replace original with optimized version
                            try FileManager.default.removeItem(at: reassembledFile)
                            try FileManager.default.moveItem(
                                at: optimizedFile, to: reassembledFile)
                            Logger.info("Using sparse-optimized file for verification")
                        } else {
                            Logger.info(
                                "Sparse optimization failed, using original file for verification")
                            try? FileManager.default.removeItem(at: optimizedFile)
                        }
                    } catch {
                        Logger.info(
                            "Error during sparse optimization: \(error.localizedDescription)")
                        try? FileManager.default.removeItem(at: optimizedFile)
                    }
                }

                // Verification step
                Logger.info("Verifying reassembled file...")
                let originalSize = diskSize
                let originalDigest = calculateSHA256(filePath: diskPath.path)
                let reassembledAttributes = try FileManager.default.attributesOfItem(
                    atPath: reassembledFile.path)
                let reassembledSize = reassembledAttributes[.size] as? UInt64 ?? 0
                let reassembledDigest = calculateSHA256(filePath: reassembledFile.path)

                // Check actual disk usage
                let originalActualSize = getActualDiskUsage(path: diskPath.path)
                let reassembledActualSize = getActualDiskUsage(path: reassembledFile.path)

                // Report results
                Logger.info("Results:")
                Logger.info(
                    "  Original size: \(ByteCountFormatter.string(fromByteCount: Int64(originalSize), countStyle: .file)) (\(originalSize) bytes)"
                )
                Logger.info(
                    "  Reassembled size: \(ByteCountFormatter.string(fromByteCount: Int64(reassembledSize), countStyle: .file)) (\(reassembledSize) bytes)"
                )
                Logger.info("  Original digest: \(originalDigest)")
                Logger.info("  Reassembled digest: \(reassembledDigest)")
                Logger.info(
                    "  Original: Apparent size: \(ByteCountFormatter.string(fromByteCount: Int64(originalSize), countStyle: .file)), Actual disk usage: \(ByteCountFormatter.string(fromByteCount: Int64(originalActualSize), countStyle: .file))"
                )
                Logger.info(
                    "  Reassembled: Apparent size: \(ByteCountFormatter.string(fromByteCount: Int64(reassembledSize), countStyle: .file)), Actual disk usage: \(ByteCountFormatter.string(fromByteCount: Int64(reassembledActualSize), countStyle: .file))"
                )

                // Determine if verification was successful
                if originalDigest == reassembledDigest {
                    Logger.info("✅ VERIFICATION SUCCESSFUL: Files are identical")
                } else {
                    Logger.info("❌ VERIFICATION FAILED: Files differ")

                    if originalSize != reassembledSize {
                        Logger.info(
                            "  Size mismatch: Original \(originalSize) bytes, Reassembled \(reassembledSize) bytes"
                        )
                    }

                    // Check sparse file characteristics
                    Logger.info("Attempting to identify differences...")
                    Logger.info(
                        "NOTE: This might be a sparse file issue. The content may be identical, but sparse regions"
                    )
                    Logger.info(
                        "      may be handled differently between the original and reassembled files."
                    )

                    if originalActualSize > 0 {
                        let diffPercentage =
                            ((Double(reassembledActualSize) - Double(originalActualSize))
                                / Double(originalActualSize)) * 100.0
                        Logger.info(
                            "  Disk usage difference: \(String(format: "%.2f", diffPercentage))%")

                        if diffPercentage < -40 {
                            Logger.info(
                                "  ⚠️ WARNING: Reassembled disk uses significantly less space (>40% difference)."
                            )
                            Logger.info(
                                "  This indicates sparse regions weren't properly preserved and may affect VM functionality."
                            )
                        } else if diffPercentage < -10 {
                            Logger.info(
                                "  ⚠️ WARNING: Reassembled disk uses less space (10-40% difference)."
                            )
                            Logger.info(
                                "  Some sparse regions may not be properly preserved but VM might still function correctly."
                            )
                        } else if diffPercentage > 10 {
                            Logger.info(
                                "  ⚠️ WARNING: Reassembled disk uses more space (>10% difference).")
                            Logger.info(
                                "  This is unusual and may indicate improper sparse file handling.")
                        } else {
                            Logger.info(
                                "  ✓ Disk usage difference is minimal (<10%). VM likely to function correctly."
                            )
                        }
                    }

                    // Offer recovery option
                    if originalDigest != reassembledDigest {
                        Logger.info("")
                        Logger.info("===== ATTEMPTING RECOVERY ACTION =====")
                        Logger.info(
                            "Since verification failed, trying direct copy as a fallback method.")

                        let fallbackFile = reassemblyDir.appendingPathComponent("fallback_disk.img")
                        Logger.info("Creating fallback disk image at: \(fallbackFile.path)")

                        // Try rsync first
                        let rsyncProcess = Process()
                        rsyncProcess.executableURL = URL(fileURLWithPath: "/usr/bin/rsync")
                        rsyncProcess.arguments = [
                            "-aS", "--progress", diskPath.path, fallbackFile.path,
                        ]

                        do {
                            try rsyncProcess.run()
                            rsyncProcess.waitUntilExit()

                            if rsyncProcess.terminationStatus == 0 {
                                Logger.info(
                                    "Direct copy completed with rsync. Fallback image available at: \(fallbackFile.path)"
                                )
                            } else {
                                // Try cp -c as fallback
                                Logger.info("Rsync failed. Attempting with cp -c command...")
                                let cpProcess = Process()
                                cpProcess.executableURL = URL(fileURLWithPath: "/bin/cp")
                                cpProcess.arguments = ["-c", diskPath.path, fallbackFile.path]

                                try cpProcess.run()
                                cpProcess.waitUntilExit()

                                if cpProcess.terminationStatus == 0 {
                                    Logger.info(
                                        "Direct copy completed with cp -c. Fallback image available at: \(fallbackFile.path)"
                                    )
                                } else {
                                    Logger.info("All recovery attempts failed.")
                                }
                            }
                        } catch {
                            Logger.info(
                                "Error during recovery attempts: \(error.localizedDescription)")
                            Logger.info("All recovery attempts failed.")
                        }
                    }
                }

                Logger.info("Reassembled file is available at: \(reassembledFile.path)")
            }
        }

        // --- Manifest Creation & Push ---
        let manifest = createManifest(
            layers: layers,
            configLayerIndex: layers.firstIndex(where: {
                $0.mediaType == "application/vnd.oci.image.config.v1+json"
            }),
            uncompressedDiskSize: uncompressedDiskSize
        )

        // Push manifest only if not in dry-run mode
        if !dryRun {
            Logger.info("Pushing manifest(s)")  // Updated log
            // Serialize the manifest dictionary to Data first
            let manifestData = try JSONSerialization.data(
                withJSONObject: manifest, options: [.prettyPrinted, .sortedKeys])

            // Loop through tags to push the same manifest data
            for tag in tags {
                Logger.info("Pushing manifest for tag: \(tag)")
                try await pushManifest(
                    repository: "\(self.organization)/\(imageName)",
                    tag: tag,  // Use the current tag from the loop
                    manifest: manifestData,  // Pass the serialized Data
                    token: token  // Token should be in scope here now
                )
            }
        }

        // Print final upload summary if not dry run
        if !dryRun {
            let stats = await uploadProgress.getUploadStats()
            Logger.info("\n\(stats.formattedSummary())")  // Add newline for separation
        }

        // Clean up cache directory only on successful non-dry-run push
    }

    private func createManifest(
        layers: [OCIManifestLayer], configLayerIndex: Int?, uncompressedDiskSize: UInt64?
    ) -> [String: Any] {
        var manifest: [String: Any] = [
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "layers": layers.map { layer in
                var layerDict: [String: Any] = [
                    "mediaType": layer.mediaType,
                    "size": layer.size,
                    "digest": layer.digest,
                ]

                if let uncompressedSize = layer.uncompressedSize {
                    var annotations: [String: String] = [:]
                    annotations["org.trycua.lume.uncompressed-size"] = "\(uncompressedSize)"  // Updated prefix

                    if let digest = layer.uncompressedContentDigest {
                        annotations["org.trycua.lume.uncompressed-content-digest"] = digest  // Updated prefix
                    }

                    layerDict["annotations"] = annotations
                }

                return layerDict
            },
        ]

        // Add config reference if available
        if let configIndex = configLayerIndex {
            let configLayer = layers[configIndex]
            manifest["config"] = [
                "mediaType": configLayer.mediaType,
                "size": configLayer.size,
                "digest": configLayer.digest,
            ]
        }

        // Add annotations
        var annotations: [String: String] = [:]
        annotations["org.trycua.lume.upload-time"] = ISO8601DateFormatter().string(from: Date())  // Updated prefix

        if let diskSize = uncompressedDiskSize {
            annotations["org.trycua.lume.uncompressed-disk-size"] = "\(diskSize)"  // Updated prefix
        }

        manifest["annotations"] = annotations

        return manifest
    }

    private func uploadBlobFromData(repository: String, data: Data, token: String) async throws
        -> String
    {
        // Calculate digest
        let digest = "sha256:" + data.sha256String()

        // Check if blob already exists
        if try await blobExists(repository: repository, digest: digest, token: token) {
            Logger.info("Blob already exists: \(digest)")
            return digest
        }

        // Initiate upload
        let uploadURL = try await startBlobUpload(repository: repository, token: token)

        // Upload blob
        try await uploadBlob(url: uploadURL, data: data, digest: digest, token: token)

        // Report progress
        await uploadProgress.addProgress(Int64(data.count))

        return digest
    }

    private func uploadBlobFromPath(repository: String, path: URL, digest: String, token: String)
        async throws -> String
    {
        // Check if blob already exists
        if try await blobExists(repository: repository, digest: digest, token: token) {
            Logger.info("Blob already exists: \(digest)")
            return digest
        }

        // Initiate upload
        let uploadURL = try await startBlobUpload(repository: repository, token: token)

        // Load data from file
        let data = try Data(contentsOf: path)

        // Upload blob
        try await uploadBlob(url: uploadURL, data: data, digest: digest, token: token)

        // Report progress
        await uploadProgress.addProgress(Int64(data.count))

        return digest
    }

    private func blobExists(repository: String, digest: String, token: String) async throws -> Bool
    {
        let url = URL(string: "https://\(registry)/v2/\(repository)/blobs/\(digest)")!
        var request = URLRequest(url: url)
        request.httpMethod = "HEAD"
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")

        let (_, response) = try await URLSession.shared.data(for: request)

        if let httpResponse = response as? HTTPURLResponse {
            return httpResponse.statusCode == 200
        }

        return false
    }

    private func startBlobUpload(repository: String, token: String) async throws -> URL {
        let url = URL(string: "https://\(registry)/v2/\(repository)/blobs/uploads/")!
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        request.setValue("0", forHTTPHeaderField: "Content-Length")  // Explicitly set Content-Length to 0 for POST

        let (_, response) = try await URLSession.shared.data(for: request)

        guard let httpResponse = response as? HTTPURLResponse,
            httpResponse.statusCode == 202,
            let locationString = httpResponse.value(forHTTPHeaderField: "Location")
        else {
            // Log response details on failure
            let responseBody =
                String(
                    data: (try? await URLSession.shared.data(for: request).0) ?? Data(),
                    encoding: .utf8) ?? "(No Body)"
            Logger.error(
                "Failed to initiate blob upload. Status: \( (response as? HTTPURLResponse)?.statusCode ?? 0 ). Headers: \( (response as? HTTPURLResponse)?.allHeaderFields ?? [:] ). Body: \(responseBody)"
            )
            throw PushError.uploadInitiationFailed
        }

        // Construct the base URL for the registry
        guard let baseRegistryURL = URL(string: "https://\(registry)") else {
            Logger.error("Failed to create base registry URL from: \(registry)")
            throw PushError.invalidURL
        }

        // Create the final upload URL, resolving the location against the base URL
        guard let uploadURL = URL(string: locationString, relativeTo: baseRegistryURL) else {
            Logger.error(
                "Failed to create absolute upload URL from location: \(locationString) relative to base: \(baseRegistryURL.absoluteString)"
            )
            throw PushError.invalidURL
        }

        Logger.info("Blob upload initiated. Upload URL: \(uploadURL.absoluteString)")
        return uploadURL.absoluteURL  // Ensure it's absolute
    }

    private func uploadBlob(url: URL, data: Data, digest: String, token: String) async throws {
        var components = URLComponents(url: url, resolvingAgainstBaseURL: true)!

        // Add digest parameter
        var queryItems = components.queryItems ?? []
        queryItems.append(URLQueryItem(name: "digest", value: digest))
        components.queryItems = queryItems

        guard let uploadURL = components.url else {
            throw PushError.invalidURL
        }

        var request = URLRequest(url: uploadURL)
        request.httpMethod = "PUT"
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        request.setValue("application/octet-stream", forHTTPHeaderField: "Content-Type")
        request.setValue("\(data.count)", forHTTPHeaderField: "Content-Length")
        request.httpBody = data

        let (_, response) = try await URLSession.shared.data(for: request)

        guard let httpResponse = response as? HTTPURLResponse, httpResponse.statusCode == 201 else {
            throw PushError.blobUploadFailed
        }
    }

    private func pushManifest(repository: String, tag: String, manifest: Data, token: String)
        async throws
    {
        let url = URL(string: "https://\(registry)/v2/\(repository)/manifests/\(tag)")!
        var request = URLRequest(url: url)
        request.httpMethod = "PUT"
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        request.setValue(
            "application/vnd.oci.image.manifest.v1+json", forHTTPHeaderField: "Content-Type")
        request.httpBody = manifest

        let (_, response) = try await URLSession.shared.data(for: request)

        guard let httpResponse = response as? HTTPURLResponse, httpResponse.statusCode == 201 else {
            throw PushError.manifestPushFailed
        }
    }

    private func getCredentialsFromEnvironment() -> (String?, String?) {
        let username =
            ProcessInfo.processInfo.environment["GITHUB_USERNAME"]
            ?? ProcessInfo.processInfo.environment["GHCR_USERNAME"]
        let password =
            ProcessInfo.processInfo.environment["GITHUB_TOKEN"]
            ?? ProcessInfo.processInfo.environment["GHCR_TOKEN"]
        return (username, password)
    }

    // Add these helper methods for dry-run and reassemble implementation

    // NEW Helper function using Compression framework and sparse writing
    private func decompressChunkAndWriteSparse(
        inputPath: String, outputHandle: FileHandle, startOffset: UInt64
    ) throws -> UInt64 {
        guard FileManager.default.fileExists(atPath: inputPath) else {
            Logger.error("Compressed chunk not found at: \(inputPath)")
            return 0  // Or throw an error
        }

        let sourceData = try Data(
            contentsOf: URL(fileURLWithPath: inputPath), options: .alwaysMapped)
        var currentWriteOffset = startOffset
        var totalDecompressedBytes: UInt64 = 0
        var sourceReadOffset = 0  // Keep track of how much compressed data we've provided

        // Use the initializer with the readingFrom closure
        let filter = try InputFilter(.decompress, using: .lz4) { (length: Int) -> Data? in
            let bytesAvailable = sourceData.count - sourceReadOffset
            if bytesAvailable == 0 {
                return nil  // No more data
            }
            let bytesToRead = min(length, bytesAvailable)
            let chunk = sourceData.subdata(in: sourceReadOffset..<sourceReadOffset + bytesToRead)
            sourceReadOffset += bytesToRead
            return chunk
        }

        // Process the decompressed output by reading from the filter
        while let decompressedData = try filter.readData(ofLength: Self.holeGranularityBytes) {
            if decompressedData.isEmpty { break }  // End of stream

            // Check if the chunk is all zeros
            if decompressedData.count == Self.holeGranularityBytes
                && decompressedData == Self.zeroChunk
            {
                // It's a zero chunk, just advance the offset, don't write
                currentWriteOffset += UInt64(decompressedData.count)
            } else {
                // Not a zero chunk (or a partial chunk at the end), write it
                try outputHandle.seek(toOffset: currentWriteOffset)
                try outputHandle.write(contentsOf: decompressedData)
                currentWriteOffset += UInt64(decompressedData.count)
            }
            totalDecompressedBytes += UInt64(decompressedData.count)
        }

        // No explicit finalize needed when initialized with source data

        return totalDecompressedBytes
    }

    // Helper function to calculate SHA256 hash of a file
    private func calculateSHA256(filePath: String) -> String {
        guard FileManager.default.fileExists(atPath: filePath) else {
            return "file-not-found"
        }

        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/shasum")
        process.arguments = ["-a", "256", filePath]

        let outputPipe = Pipe()
        process.standardOutput = outputPipe

        do {
            try process.run()
            process.waitUntilExit()

            if let data = try outputPipe.fileHandleForReading.readToEnd(),
                let output = String(data: data, encoding: .utf8)
            {
                return output.components(separatedBy: " ").first ?? "hash-calculation-failed"
            }
        } catch {
            Logger.error("SHA256 calculation failed: \(error)")
        }

        return "hash-calculation-failed"
    }
}

actor UploadProgressTracker {
    private var totalBytes: Int64 = 0
    private var uploadedBytes: Int64 = 0  // Renamed
    private var progressLogger = ProgressLogger(threshold: 0.01)
    private var totalFiles: Int = 0  // Keep track of total items
    private var completedFiles: Int = 0  // Keep track of completed items

    // Upload speed tracking
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
        uploadedBytes = 0  // Reset uploaded bytes
        completedFiles = 0  // Reset completed files
        smoothedSpeed = 0
        speedSamples = []
        peakSpeed = 0
        totalElapsedTime = 0
    }

    func addProgress(_ bytes: Int64) {
        uploadedBytes += bytes
        completedFiles += 1  // Increment completed files count
        let now = Date()
        let elapsed = now.timeIntervalSince(lastUpdateTime)

        // Show first progress update immediately, then throttle updates
        let shouldUpdate =
            (uploadedBytes <= bytes) || (elapsed >= 0.5) || (completedFiles == totalFiles)

        if shouldUpdate && totalBytes > 0 {  // Ensure totalBytes is set
            let currentSpeed = Double(uploadedBytes - lastUpdateBytes) / max(elapsed, 0.001)
            speedSamples.append(currentSpeed)

            // Cap samples array
            if speedSamples.count > 20 {
                speedSamples.removeFirst(speedSamples.count - 20)
            }

            peakSpeed = max(peakSpeed, currentSpeed)

            // Apply exponential smoothing
            if smoothedSpeed == 0 {
                smoothedSpeed = currentSpeed
            } else {
                smoothedSpeed = speedSmoothing * currentSpeed + (1 - speedSmoothing) * smoothedSpeed
            }

            let recentAvgSpeed = calculateAverageSpeed()
            let totalElapsed = now.timeIntervalSince(startTime)
            let overallAvgSpeed = totalElapsed > 0 ? Double(uploadedBytes) / totalElapsed : 0

            let progress = totalBytes > 0 ? Double(uploadedBytes) / Double(totalBytes) : 1.0  // Avoid division by zero
            logSpeedProgress(
                current: progress,
                currentSpeed: currentSpeed,
                averageSpeed: recentAvgSpeed,
                smoothedSpeed: smoothedSpeed,
                overallSpeed: overallAvgSpeed,
                peakSpeed: peakSpeed,
                context: "Uploading Image"  // Changed context
            )

            lastUpdateTime = now
            lastUpdateBytes = uploadedBytes
            totalElapsedTime = totalElapsed
        }
    }

    private func calculateAverageSpeed() -> Double {
        guard !speedSamples.isEmpty else { return 0 }
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

    // Use the UploadStats struct
    func getUploadStats() -> UploadStats {
        let avgSpeed = totalElapsedTime > 0 ? Double(uploadedBytes) / totalElapsedTime : 0
        return UploadStats(
            totalBytes: totalBytes,
            uploadedBytes: uploadedBytes,  // Renamed
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
        // let currentSpeedStr = formatByteSpeed(currentSpeed) // Removed unused
        let avgSpeedStr = formatByteSpeed(averageSpeed)
        // let peakSpeedStr = formatByteSpeed(peakSpeed) // Removed unused
        let remainingBytes = totalBytes - uploadedBytes
        let speedForEta = max(smoothedSpeed, averageSpeed * 0.8)
        let etaSeconds = speedForEta > 0 ? Double(remainingBytes) / speedForEta : 0
        let etaStr = formatTimeRemaining(etaSeconds)
        let progressBar = createProgressBar(progress: current)
        let fileProgress = "(\(completedFiles)/\(totalFiles))"  // Add file count

        print(
            "\r\(progressBar) \(progressPercent)% \(fileProgress) | Speed: \(avgSpeedStr) (Avg) | ETA: \(etaStr)     ",  // Simplified output
            terminator: "")
        fflush(stdout)
    }

    // Helper methods (createProgressBar, formatByteSpeed, formatTimeRemaining) remain the same
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
        if seconds.isNaN || seconds.isInfinite || seconds <= 0 { return "calculating..." }
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
