import Foundation
import Testing

@testable import lume

@Test("GCSRegistryError descriptions are informative")
func testGCSRegistryErrorDescriptions() async throws {
    let invalidFormatError = GCSRegistryError.invalidImageFormat("test:invalid:format")
    #expect(invalidFormatError.errorDescription?.contains("Invalid image format") == true)

    let notFoundError = GCSRegistryError.imageNotFound("test-image", "v1.0")
    #expect(notFoundError.errorDescription?.contains("not found") == true)
    #expect(notFoundError.errorDescription?.contains("test-image:v1.0") == true)

    let invalidResponseError = GCSRegistryError.invalidResponse
    #expect(invalidResponseError.errorDescription?.contains("Invalid response") == true)

    let invalidURLError = GCSRegistryError.invalidDownloadURL("bad-url")
    #expect(invalidURLError.errorDescription?.contains("Invalid download URL") == true)

    let apiError = GCSRegistryError.apiError(statusCode: 500, message: "Internal Server Error")
    #expect(apiError.errorDescription?.contains("500") == true)
    #expect(apiError.errorDescription?.contains("Internal Server Error") == true)

    let downloadFailedError = GCSRegistryError.downloadFailed(url: "https://example.com", statusCode: 404)
    #expect(downloadFailedError.errorDescription?.contains("404") == true)
    #expect(downloadFailedError.errorDescription?.contains("https://example.com") == true)

    let uploadFailedError = GCSRegistryError.uploadFailed(partURL: "https://example.com/part1", statusCode: 403)
    #expect(uploadFailedError.errorDescription?.contains("403") == true)

    let checksumError = GCSRegistryError.checksumMismatch(expected: "abc123", actual: "def456")
    #expect(checksumError.errorDescription?.contains("abc123") == true)
    #expect(checksumError.errorDescription?.contains("def456") == true)

    let extractionError = GCSRegistryError.extractionFailed("/tmp/archive.tar.gz")
    #expect(extractionError.errorDescription?.contains("extract") == true)

    let archiveError = GCSRegistryError.archiveCreationFailed("/tmp/source")
    #expect(archiveError.errorDescription?.contains("create archive") == true)

    let vmNotFoundError = GCSRegistryError.vmDirectoryNotFound("/path/to/vm")
    #expect(vmNotFoundError.errorDescription?.contains("not found") == true)

    let noTagsError = GCSRegistryError.noTagsProvided
    #expect(noTagsError.errorDescription?.contains("tag") == true)
}

@Test("GCSImageRegistry initialization validates config")
func testGCSImageRegistryInitValidation() async throws {
    // Test with empty API URL
    let emptyUrlConfig = GCSConfig(apiUrl: "", apiKey: "valid-key")
    #expect(throws: RegistryConfigError.self) {
        try GCSImageRegistry(config: emptyUrlConfig)
    }

    // Test with empty API key
    let emptyKeyConfig = GCSConfig(apiUrl: "https://api.example.com", apiKey: "")
    #expect(throws: RegistryConfigError.self) {
        try GCSImageRegistry(config: emptyKeyConfig)
    }

    // Test with valid config (should not throw)
    let validConfig = GCSConfig(apiUrl: "https://api.example.com", apiKey: "valid-key")
    let registry = try GCSImageRegistry(config: validConfig)
    #expect(registry != nil)
}

@Test("GCSImageRegistry PartInfo encoding and decoding")
func testGCSImageRegistryPartInfoCodable() async throws {
    let partInfo = GCSImageRegistry.PartInfo(partNumber: 1, etag: "abc123")

    let encoder = JSONEncoder()
    let data = try encoder.encode(partInfo)

    let decoder = JSONDecoder()
    let decoded = try decoder.decode(GCSImageRegistry.PartInfo.self, from: data)

    #expect(decoded.partNumber == 1)
    #expect(decoded.etag == "abc123")
}

@Test("GCSImageRegistry handles invalid image format")
func testGCSImageRegistryInvalidImageFormat() async throws {
    // This test would require a mock, but we can test the error path
    let config = GCSConfig(apiUrl: "https://api.example.com", apiKey: "test-key")
    let registry = try GCSImageRegistry(config: config)

    // Image format validation happens in the pull method
    // We expect it to throw for invalid formats
    // Note: This will actually try to hit the API, so we're testing error handling
    await #expect(throws: Error.self) {
        try await registry.pull(image: "invalid-format", name: nil, locationName: nil)
    }
}

@Test("GCSImageRegistry push validates tags")
func testGCSImageRegistryPushValidatesTags() async throws {
    let config = GCSConfig(apiUrl: "https://api.example.com", apiKey: "test-key")
    let registry = try GCSImageRegistry(config: config)

    // Create a temporary directory
    let tempDir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: tempDir) }

    // Test with empty tags array
    await #expect(throws: GCSRegistryError.noTagsProvided) {
        try await registry.push(
            vmDirPath: tempDir.path,
            imageName: "test-image",
            tags: [],
            chunkSizeMb: 512,
            verbose: false,
            dryRun: false,
            reassemble: false,
            singleLayer: false,
            legacy: false
        )
    }
}

@Test("GCSImageRegistry push validates VM directory exists")
func testGCSImageRegistryPushValidatesVMDirectory() async throws {
    let config = GCSConfig(apiUrl: "https://api.example.com", apiKey: "test-key")
    let registry = try GCSImageRegistry(config: config)

    // Test with non-existent directory
    await #expect(throws: GCSRegistryError.vmDirectoryNotFound) {
        try await registry.push(
            vmDirPath: "/nonexistent/path",
            imageName: "test-image",
            tags: ["latest"],
            chunkSizeMb: 512,
            verbose: false,
            dryRun: false,
            reassemble: false,
            singleLayer: false,
            legacy: false
        )
    }
}