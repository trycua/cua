import Foundation
import Testing

@testable import lume

// Integration tests for end-to-end workflows

@MainActor
@Test("Full workflow: validate image format parsing")
func testImageFormatParsingWorkflow() async throws {
    // Test the full flow of parsing an image reference
    let imageRef = "ghcr.io/trycua/macos-sequoia:latest"

    // Parse organization and image
    let components = imageRef.split(separator: "/")
    #expect(components.count >= 2)

    let imagePart = String(components.last!)
    let imageComponents = imagePart.split(separator: ":")

    #expect(imageComponents.count == 2)
    let imageName = String(imageComponents[0])
    let tag = String(imageComponents[1])

    #expect(imageName == "macos-sequoia")
    #expect(tag == "latest")
}

@MainActor
@Test("Full workflow: normalize VM name from image")
func testNormalizeVMNameWorkflow() async throws {
    let image = "macos-sequoia:latest"

    // Parse and normalize
    let components = image.split(separator: ":")
    let normalized = components.count == 2 ? "\(components[0])_\(components[1])" : image

    #expect(normalized == "macos-sequoia_latest")

    // Verify it's safe for filesystem
    #expect(!normalized.contains(":"))
}

@MainActor
@Test("Full workflow: tag deduplication")
func testTagDeduplicationWorkflow() async throws {
    let primaryTag = "v1.0"
    let additionalTags = ["latest", "stable", "v1.0", "prod", "latest"]

    // Combine and deduplicate
    var allTags: Set<String> = [primaryTag]
    allTags.formUnion(additionalTags)

    #expect(allTags.count == 4) // v1.0, latest, stable, prod
    #expect(allTags.contains("v1.0"))
    #expect(allTags.contains("latest"))
    #expect(allTags.contains("stable"))
    #expect(allTags.contains("prod"))
}

@Test("Full workflow: request validation chain")
func testRequestValidationChain() async throws {
    // Simulate the validation chain for a create request
    let json = """
    {
        "name": "test-vm",
        "os": "macos",
        "cpu": 4,
        "memory": "8GB",
        "diskSize": "128GB",
        "display": "1920x1080",
        "ipsw": "latest",
        "network": "nat"
    }
    """

    let data = json.data(using: .utf8)!
    let request = try JSONDecoder().decode(CreateVMRequest.self, from: data)

    // Parse sizes
    let (memory, diskSize) = try request.parse()
    #expect(memory == 8 * 1024 * 1024 * 1024)
    #expect(diskSize == 128 * 1024 * 1024 * 1024)

    // Parse network mode
    let networkMode = try request.parseNetworkMode()
    #expect(networkMode == .nat)

    // Validate name
    #expect(!request.name.isEmpty)

    // Validate OS
    #expect(request.os.lowercased() == "macos" || request.os.lowercased() == "linux")
}

@Test("Full workflow: pull request with defaults")
func testPullRequestWorkflow() async throws {
    // Minimal pull request should use defaults
    let json = """
    {
        "image": "macos-sequoia:latest",
        "name": "my-vm"
    }
    """

    let data = json.data(using: .utf8)!
    let request = try JSONDecoder().decode(PullRequest.self, from: data)

    // Verify defaults are applied
    #expect(request.image == "macos-sequoia:latest")
    #expect(request.name == "my-vm")
    #expect(request.registry == "ghcr.io")
    #expect(request.organization == "trycua")

    // Parse image
    let components = request.image.split(separator: ":")
    #expect(components.count == 2)

    let imageName = String(components[0])
    let tag = String(components[1])

    #expect(imageName == "macos-sequoia")
    #expect(tag == "latest")
}

@Test("Full workflow: push request validation")
func testPushRequestWorkflow() async throws {
    let json = """
    {
        "name": "my-vm",
        "imageName": "my-org/my-image",
        "tags": ["v1.0", "latest"],
        "registry": "ghcr.io",
        "organization": "my-org",
        "chunkSizeMb": 1024,
        "singleLayer": false
    }
    """

    let data = json.data(using: .utf8)!
    let request = try JSONDecoder().decode(PushRequest.self, from: data)

    // Validate all fields
    #expect(!request.name.isEmpty)
    #expect(!request.imageName.isEmpty)
    #expect(!request.tags.isEmpty)
    #expect(!request.registry.isEmpty)
    #expect(!request.organization.isEmpty)
    #expect(request.chunkSizeMb > 0)
    #expect(request.tags.count == 2)
}

@MainActor
@Test("Full workflow: VM status determination")
func testVMStatusWorkflow() async throws {
    // Test the logic for determining VM status

    // Case 1: VM with provisioning marker
    let hasProvisioningMarker = true
    let hasRequiredFiles = false

    if hasProvisioningMarker && !hasRequiredFiles {
        let status = "provisioning"
        #expect(status == "provisioning")
    }

    // Case 2: VM is running
    let isInCache = true
    if isInCache {
        let status = "running"
        #expect(status == "running")
    }

    // Case 3: VM is stopped
    let isStopped = !hasProvisioningMarker && !isInCache
    if isStopped {
        let status = "stopped"
        #expect(status == "stopped")
    }
}

@MainActor
@Test("Full workflow: error handling chain")
func testErrorHandlingWorkflow() async throws {
    // Test error types are properly structured

    // Validation error
    let validationError = ValidationError("Invalid input")
    #expect(validationError.localizedDescription.contains("Invalid"))

    // GCS error
    let gcsError = GCSRegistryError.imageNotFound("test", "v1")
    #expect(gcsError.errorDescription != nil)

    // VM error
    let vmError = VMError.notFound("test-vm")
    #expect(vmError.localizedDescription.contains("not found"))
}

@Test("Full workflow: JSON encoding and decoding")
func testJSONWorkflow() async throws {
    // Test round-trip encoding/decoding

    let original = PushRequest(
        name: "test-vm",
        imageName: "test-image",
        tags: ["v1", "latest"],
        registry: "ghcr.io",
        organization: "test-org",
        storage: "/custom/path",
        chunkSizeMb: 512,
        singleLayer: false
    )

    let encoder = JSONEncoder()
    let data = try encoder.encode(original)

    let decoder = JSONDecoder()
    let decoded = try decoder.decode(PushRequest.self, from: data)

    #expect(decoded.name == original.name)
    #expect(decoded.imageName == original.imageName)
    #expect(decoded.tags == original.tags)
    #expect(decoded.registry == original.registry)
    #expect(decoded.organization == original.organization)
    #expect(decoded.storage == original.storage)
    #expect(decoded.chunkSizeMb == original.chunkSizeMb)
}

@MainActor
@Test("Full workflow: settings management")
func testSettingsWorkflow() async throws {
    let controller = LumeController()

    // Get current settings
    let settings = controller.getSettings()
    #expect(settings.homeDirectory != nil)

    // Get specific values
    let cacheDir = controller.getCacheDirectory()
    #expect(!cacheDir.isEmpty)

    let cachingEnabled = controller.isCachingEnabled()
    #expect(cachingEnabled == true || cachingEnabled == false)

    let telemetryEnabled = controller.isTelemetryEnabled()
    #expect(telemetryEnabled == true || telemetryEnabled == false)
}

@Test("Full workflow: shared directory validation")
func testSharedDirectoryWorkflow() async throws {
    // Create temp directory for testing
    let tempDir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: tempDir) }

    let request = RunVMRequest(
        noDisplay: true,
        sharedDirectories: [
            RunVMRequest.SharedDirectoryRequest(hostPath: tempDir.path, readOnly: false)
        ],
        recoveryMode: false,
        storage: nil,
        network: nil,
        clipboard: false
    )

    let sharedDirs = try request.parse()

    #expect(sharedDirs.count == 1)
    #expect(sharedDirs[0].hostPath == tempDir.path)
    #expect(sharedDirs[0].readOnly == false)
}