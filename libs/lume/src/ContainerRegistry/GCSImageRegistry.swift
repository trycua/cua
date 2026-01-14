import CommonCrypto
import Foundation

/// GCS-backed image registry that uses Richmond API for signed URLs
class GCSImageRegistry: ImageRegistry, @unchecked Sendable {
    private let config: GCSConfig
    private let urlSession: URLSession

    /// Chunk size for multipart uploads (100MB as per Richmond API)
    private static let chunkSize: Int = 100 * 1024 * 1024

    init(config: GCSConfig) throws {
        guard !config.apiUrl.isEmpty else {
            throw RegistryConfigError.invalidApiUrl(config.apiUrl)
        }
        guard !config.apiKey.isEmpty else {
            throw RegistryConfigError.missingApiKey
        }
        self.config = config

        // Configure URLSession with reasonable timeouts
        let sessionConfig = URLSessionConfiguration.default
        sessionConfig.timeoutIntervalForRequest = 300 // 5 minutes
        sessionConfig.timeoutIntervalForResource = 3600 // 1 hour for large downloads
        self.urlSession = URLSession(configuration: sessionConfig)
    }

    // MARK: - ImageRegistry Protocol

    func pull(
        image: String,
        name: String?,
        locationName: String?
    ) async throws -> VMDirectory {
        // Parse image name and tag
        let components = image.split(separator: ":")
        guard components.count == 2 else {
            throw GCSRegistryError.invalidImageFormat(image)
        }

        let imageName = String(components[0])
        let imageTag = String(components[1])
        let vmName = name ?? imageName

        Logger.info(
            "Pulling image from GCS",
            metadata: [
                "image": image,
                "name": vmName,
                "api_url": config.apiUrl
            ])

        // Get the VM directory
        let home = Home()
        let vmDir: VMDirectory
        if let locationName = locationName,
           locationName.contains("/") || locationName.contains("\\") {
            vmDir = try home.getVMDirectoryFromPath(vmName, storagePath: locationName)
        } else {
            vmDir = try home.getVMDirectory(vmName, storage: locationName)
        }

        // Get signed download URL from Richmond API
        Logger.info("Getting download URL from API")
        let downloadInfo = try await getDownloadURL(imageName: imageName, tag: imageTag)

        Logger.info(
            "Got download URL",
            metadata: [
                "size_bytes": "\(downloadInfo.sizeBytes)",
                "expires_in": "\(downloadInfo.expiresIn)s"
            ])

        // Create temporary directory for download
        let tempDir = FileManager.default.temporaryDirectory.appendingPathComponent("lume_gcs_\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer {
            try? FileManager.default.removeItem(at: tempDir)
        }

        let archivePath = tempDir.appendingPathComponent("image.tar.gz")

        // Download the image
        Logger.info("Downloading image from GCS")
        try await downloadFile(from: downloadInfo.url, to: archivePath, expectedSize: downloadInfo.sizeBytes)

        // Verify checksum if provided
        if let expectedChecksum = downloadInfo.checksum {
            Logger.info("Verifying checksum")
            let actualChecksum = try calculateSHA256(fileAt: archivePath)
            guard actualChecksum == expectedChecksum else {
                throw GCSRegistryError.checksumMismatch(expected: expectedChecksum, actual: actualChecksum)
            }
            Logger.info("Checksum verified")
        }

        // Ensure VM directory exists
        try FileManager.default.createDirectory(at: vmDir.dir.url, withIntermediateDirectories: true)

        // Extract the archive to VM directory
        Logger.info("Extracting image to VM directory")
        try await extractArchive(archivePath, to: vmDir.dir.url)

        Logger.info("Pull complete", metadata: ["vm_path": vmDir.dir.path])

        return vmDir
    }

    func push(
        vmDirPath: String,
        imageName: String,
        tags: [String],
        chunkSizeMb: Int,
        verbose: Bool,
        dryRun: Bool,
        reassemble: Bool
    ) async throws {
        guard !tags.isEmpty else {
            throw GCSRegistryError.noTagsProvided
        }

        let vmDir = URL(fileURLWithPath: vmDirPath)
        guard FileManager.default.fileExists(atPath: vmDirPath) else {
            throw GCSRegistryError.vmDirectoryNotFound(vmDirPath)
        }

        Logger.info(
            "Pushing VM to GCS",
            metadata: [
                "vm_path": vmDirPath,
                "image_name": imageName,
                "tags": tags.joined(separator: ", "),
                "api_url": config.apiUrl
            ])

        // Create temporary directory for the archive
        let tempDir = FileManager.default.temporaryDirectory.appendingPathComponent("lume_gcs_push_\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer {
            try? FileManager.default.removeItem(at: tempDir)
        }

        let archivePath = tempDir.appendingPathComponent("image.tar.gz")

        // Create tar.gz archive of the VM directory
        Logger.info("Creating archive")
        try await createArchive(from: vmDir, to: archivePath)

        // Calculate size and checksum
        let fileAttributes = try FileManager.default.attributesOfItem(atPath: archivePath.path)
        let fileSize = fileAttributes[.size] as! Int64
        let checksum = try calculateSHA256(fileAt: archivePath)

        Logger.info(
            "Archive created",
            metadata: [
                "size_bytes": "\(fileSize)",
                "checksum": checksum
            ])

        if dryRun {
            Logger.info("Dry run mode - skipping upload")
            return
        }

        // Push to each tag
        for tag in tags {
            Logger.info("Uploading image", metadata: ["tag": tag])

            // Initiate upload
            let uploadSession = try await initiateUpload(
                imageName: imageName,
                tag: tag,
                sizeBytes: fileSize,
                checksum: checksum
            )

            Logger.info("Upload initiated", metadata: ["upload_id": uploadSession.uploadId])

            do {
                // Upload in chunks
                let parts = try await uploadInChunks(
                    imageName: imageName,
                    uploadId: uploadSession.uploadId,
                    filePath: archivePath,
                    fileSize: fileSize
                )

                // Complete upload
                try await completeUpload(
                    imageName: imageName,
                    uploadId: uploadSession.uploadId,
                    parts: parts
                )

                Logger.info("Upload complete", metadata: ["tag": tag])
            } catch {
                // Abort upload on failure
                Logger.error("Upload failed, aborting", metadata: ["error": "\(error)"])
                try? await abortUpload(imageName: imageName, uploadId: uploadSession.uploadId)
                throw error
            }
        }

        Logger.info("Push complete")
    }

    func getImages() async throws -> [CachedImage] {
        // Call Richmond API to list images
        let url = URL(string: "\(config.apiUrl)/v1/images")!
        var request = URLRequest(url: url)
        request.setValue(config.apiKey, forHTTPHeaderField: "X-API-Key")

        let (data, response) = try await urlSession.data(for: request)

        guard let httpResponse = response as? HTTPURLResponse else {
            throw GCSRegistryError.invalidResponse
        }

        guard httpResponse.statusCode == 200 else {
            throw GCSRegistryError.apiError(statusCode: httpResponse.statusCode, message: String(data: data, encoding: .utf8) ?? "Unknown error")
        }

        struct ListResponse: Codable {
            let images: [ImageInfo]
        }

        struct ImageInfo: Codable {
            let name: String
            let versions: [VersionInfo]?
        }

        struct VersionInfo: Codable {
            let tag: String
            let sizeBytes: Int64?
            let createdAt: String?

            enum CodingKeys: String, CodingKey {
                case tag
                case sizeBytes = "size_bytes"
                case createdAt = "created_at"
            }
        }

        let listResponse = try JSONDecoder().decode(ListResponse.self, from: data)

        // Convert to CachedImage format
        var images: [CachedImage] = []
        for imageInfo in listResponse.images {
            for version in imageInfo.versions ?? [] {
                images.append(CachedImage(
                    repository: imageInfo.name,
                    imageId: "\(imageInfo.name):\(version.tag)",
                    manifestId: "\(imageInfo.name):\(version.tag)"
                ))
            }
        }

        return images
    }

    // MARK: - Private API Methods

    private struct DownloadInfo {
        let url: URL
        let sizeBytes: Int64
        let checksum: String?
        let expiresIn: Int
    }

    private func getDownloadURL(imageName: String, tag: String) async throws -> DownloadInfo {
        var urlComponents = URLComponents(string: "\(config.apiUrl)/v1/images/\(imageName)/download")!
        urlComponents.queryItems = [URLQueryItem(name: "tag", value: tag)]

        var request = URLRequest(url: urlComponents.url!)
        request.setValue(config.apiKey, forHTTPHeaderField: "X-API-Key")

        let (data, response) = try await urlSession.data(for: request)

        guard let httpResponse = response as? HTTPURLResponse else {
            throw GCSRegistryError.invalidResponse
        }

        guard httpResponse.statusCode == 200 else {
            if httpResponse.statusCode == 404 {
                throw GCSRegistryError.imageNotFound(imageName, tag)
            }
            throw GCSRegistryError.apiError(statusCode: httpResponse.statusCode, message: String(data: data, encoding: .utf8) ?? "Unknown error")
        }

        struct Response: Codable {
            let downloadUrl: String
            let expiresIn: Int
            let sizeBytes: Int64
            let checksumSha256: String?

            enum CodingKeys: String, CodingKey {
                case downloadUrl = "download_url"
                case expiresIn = "expires_in"
                case sizeBytes = "size_bytes"
                case checksumSha256 = "checksum_sha256"
            }
        }

        let downloadResponse = try JSONDecoder().decode(Response.self, from: data)

        guard let downloadURL = URL(string: downloadResponse.downloadUrl) else {
            throw GCSRegistryError.invalidDownloadURL(downloadResponse.downloadUrl)
        }

        return DownloadInfo(
            url: downloadURL,
            sizeBytes: downloadResponse.sizeBytes,
            checksum: downloadResponse.checksumSha256,
            expiresIn: downloadResponse.expiresIn
        )
    }

    private struct UploadSession {
        let uploadId: String
        let partSize: Int
    }

    private func initiateUpload(imageName: String, tag: String, sizeBytes: Int64, checksum: String) async throws -> UploadSession {
        let url = URL(string: "\(config.apiUrl)/v1/images/\(imageName)/upload")!
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.setValue(config.apiKey, forHTTPHeaderField: "X-API-Key")
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")

        let body: [String: Any] = [
            "tag": tag,
            "image_type": "lume",
            "size_bytes": sizeBytes,
            "checksum_sha256": checksum
        ]
        request.httpBody = try JSONSerialization.data(withJSONObject: body)

        let (data, response) = try await urlSession.data(for: request)

        guard let httpResponse = response as? HTTPURLResponse else {
            throw GCSRegistryError.invalidResponse
        }

        guard httpResponse.statusCode == 200 else {
            throw GCSRegistryError.apiError(statusCode: httpResponse.statusCode, message: String(data: data, encoding: .utf8) ?? "Unknown error")
        }

        struct Response: Codable {
            let uploadId: String
            let partSize: Int?

            enum CodingKeys: String, CodingKey {
                case uploadId = "upload_id"
                case partSize = "part_size"
            }
        }

        let uploadResponse = try JSONDecoder().decode(Response.self, from: data)

        return UploadSession(
            uploadId: uploadResponse.uploadId,
            partSize: uploadResponse.partSize ?? GCSImageRegistry.chunkSize
        )
    }

    private struct PartInfo: Codable {
        let partNumber: Int
        let etag: String

        enum CodingKeys: String, CodingKey {
            case partNumber = "part_number"
            case etag
        }
    }

    private func uploadInChunks(imageName: String, uploadId: String, filePath: URL, fileSize: Int64) async throws -> [PartInfo] {
        let fileHandle = try FileHandle(forReadingFrom: filePath)
        defer { try? fileHandle.close() }

        var parts: [PartInfo] = []
        var partNumber = 1
        var bytesUploaded: Int64 = 0

        while bytesUploaded < fileSize {
            // Get signed URL for this part
            let partURL = try await getPartUploadURL(imageName: imageName, uploadId: uploadId, partNumber: partNumber)

            // Read chunk
            let chunkSize = min(GCSImageRegistry.chunkSize, Int(fileSize - bytesUploaded))
            guard let chunkData = try fileHandle.read(upToCount: chunkSize) else {
                break
            }

            // Upload chunk
            let etag = try await uploadPart(url: partURL, data: chunkData)

            parts.append(PartInfo(partNumber: partNumber, etag: etag))
            bytesUploaded += Int64(chunkData.count)
            partNumber += 1

            let progress = Double(bytesUploaded) / Double(fileSize) * 100
            Logger.info("Upload progress: \(String(format: "%.1f", progress))%")
        }

        return parts
    }

    private func getPartUploadURL(imageName: String, uploadId: String, partNumber: Int) async throws -> URL {
        let url = URL(string: "\(config.apiUrl)/v1/images/\(imageName)/upload/\(uploadId)/part/\(partNumber)")!
        var request = URLRequest(url: url)
        request.setValue(config.apiKey, forHTTPHeaderField: "X-API-Key")

        let (data, response) = try await urlSession.data(for: request)

        guard let httpResponse = response as? HTTPURLResponse else {
            throw GCSRegistryError.invalidResponse
        }

        guard httpResponse.statusCode == 200 else {
            throw GCSRegistryError.apiError(statusCode: httpResponse.statusCode, message: String(data: data, encoding: .utf8) ?? "Unknown error")
        }

        struct Response: Codable {
            let uploadUrl: String

            enum CodingKeys: String, CodingKey {
                case uploadUrl = "upload_url"
            }
        }

        let partResponse = try JSONDecoder().decode(Response.self, from: data)

        guard let partURL = URL(string: partResponse.uploadUrl) else {
            throw GCSRegistryError.invalidDownloadURL(partResponse.uploadUrl)
        }

        return partURL
    }

    private func uploadPart(url: URL, data: Data) async throws -> String {
        var request = URLRequest(url: url)
        request.httpMethod = "PUT"
        request.setValue("application/octet-stream", forHTTPHeaderField: "Content-Type")
        request.setValue("\(data.count)", forHTTPHeaderField: "Content-Length")

        let (_, response) = try await urlSession.upload(for: request, from: data)

        guard let httpResponse = response as? HTTPURLResponse else {
            throw GCSRegistryError.invalidResponse
        }

        guard httpResponse.statusCode == 200 || httpResponse.statusCode == 201 else {
            throw GCSRegistryError.uploadFailed(partURL: url.absoluteString, statusCode: httpResponse.statusCode)
        }

        // GCS returns ETag in response header
        return httpResponse.value(forHTTPHeaderField: "ETag") ?? ""
    }

    private func completeUpload(imageName: String, uploadId: String, parts: [PartInfo]) async throws {
        let url = URL(string: "\(config.apiUrl)/v1/images/\(imageName)/upload/\(uploadId)/complete")!
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.setValue(config.apiKey, forHTTPHeaderField: "X-API-Key")
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")

        let body = ["parts": parts.map { ["part_number": $0.partNumber, "etag": $0.etag] }]
        request.httpBody = try JSONSerialization.data(withJSONObject: body)

        let (data, response) = try await urlSession.data(for: request)

        guard let httpResponse = response as? HTTPURLResponse else {
            throw GCSRegistryError.invalidResponse
        }

        guard httpResponse.statusCode == 200 else {
            throw GCSRegistryError.apiError(statusCode: httpResponse.statusCode, message: String(data: data, encoding: .utf8) ?? "Unknown error")
        }
    }

    private func abortUpload(imageName: String, uploadId: String) async throws {
        let url = URL(string: "\(config.apiUrl)/v1/images/\(imageName)/upload/\(uploadId)")!
        var request = URLRequest(url: url)
        request.httpMethod = "DELETE"
        request.setValue(config.apiKey, forHTTPHeaderField: "X-API-Key")

        let (_, _) = try await urlSession.data(for: request)
    }

    // MARK: - Private Helper Methods

    private func downloadFile(from url: URL, to destination: URL, expectedSize: Int64) async throws {
        let (tempURL, response) = try await urlSession.download(from: url)

        guard let httpResponse = response as? HTTPURLResponse else {
            throw GCSRegistryError.invalidResponse
        }

        guard httpResponse.statusCode == 200 else {
            throw GCSRegistryError.downloadFailed(url: url.absoluteString, statusCode: httpResponse.statusCode)
        }

        // Move to destination
        if FileManager.default.fileExists(atPath: destination.path) {
            try FileManager.default.removeItem(at: destination)
        }
        try FileManager.default.moveItem(at: tempURL, to: destination)

        // Verify size
        let attributes = try FileManager.default.attributesOfItem(atPath: destination.path)
        let actualSize = attributes[.size] as! Int64
        if actualSize != expectedSize {
            Logger.info("Downloaded file size mismatch", metadata: [
                "expected": "\(expectedSize)",
                "actual": "\(actualSize)"
            ])
        }
    }

    private func calculateSHA256(fileAt path: URL) throws -> String {
        let fileHandle = try FileHandle(forReadingFrom: path)
        defer { try? fileHandle.close() }

        var context = CC_SHA256_CTX()
        CC_SHA256_Init(&context)

        let bufferSize = 1024 * 1024 // 1MB chunks
        while autoreleasepool(invoking: {
            guard let data = try? fileHandle.read(upToCount: bufferSize), !data.isEmpty else {
                return false
            }
            data.withUnsafeBytes { bytes in
                _ = CC_SHA256_Update(&context, bytes.baseAddress, CC_LONG(data.count))
            }
            return true
        }) {}

        var digest = [UInt8](repeating: 0, count: Int(CC_SHA256_DIGEST_LENGTH))
        CC_SHA256_Final(&digest, &context)

        return digest.map { String(format: "%02x", $0) }.joined()
    }

    private func extractArchive(_ archive: URL, to destination: URL) async throws {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/tar")
        process.arguments = ["-xzf", archive.path, "-C", destination.path]

        try process.run()
        process.waitUntilExit()

        guard process.terminationStatus == 0 else {
            throw GCSRegistryError.extractionFailed(archive.path)
        }
    }

    private func createArchive(from source: URL, to destination: URL) async throws {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/tar")
        process.currentDirectoryURL = source.deletingLastPathComponent()
        process.arguments = ["-czf", destination.path, source.lastPathComponent]

        try process.run()
        process.waitUntilExit()

        guard process.terminationStatus == 0 else {
            throw GCSRegistryError.archiveCreationFailed(source.path)
        }
    }
}

// MARK: - Errors

public enum GCSRegistryError: Error, LocalizedError {
    case invalidImageFormat(String)
    case imageNotFound(String, String)
    case invalidResponse
    case invalidDownloadURL(String)
    case apiError(statusCode: Int, message: String)
    case downloadFailed(url: String, statusCode: Int)
    case uploadFailed(partURL: String, statusCode: Int)
    case checksumMismatch(expected: String, actual: String)
    case extractionFailed(String)
    case archiveCreationFailed(String)
    case vmDirectoryNotFound(String)
    case noTagsProvided

    public var errorDescription: String? {
        switch self {
        case .invalidImageFormat(let image):
            return "Invalid image format '\(image)'. Expected format: name:tag"
        case .imageNotFound(let name, let tag):
            return "Image '\(name):\(tag)' not found in GCS registry"
        case .invalidResponse:
            return "Invalid response from API"
        case .invalidDownloadURL(let url):
            return "Invalid download URL: \(url)"
        case .apiError(let statusCode, let message):
            return "API error (HTTP \(statusCode)): \(message)"
        case .downloadFailed(let url, let statusCode):
            return "Download failed (HTTP \(statusCode)) from: \(url)"
        case .uploadFailed(let partURL, let statusCode):
            return "Upload failed (HTTP \(statusCode)) to: \(partURL)"
        case .checksumMismatch(let expected, let actual):
            return "Checksum mismatch. Expected: \(expected), Got: \(actual)"
        case .extractionFailed(let path):
            return "Failed to extract archive: \(path)"
        case .archiveCreationFailed(let path):
            return "Failed to create archive from: \(path)"
        case .vmDirectoryNotFound(let path):
            return "VM directory not found: \(path)"
        case .noTagsProvided:
            return "At least one tag must be provided"
        }
    }
}

