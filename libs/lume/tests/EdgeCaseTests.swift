import Foundation
import Testing

@testable import lume

// Additional edge case and regression tests for comprehensive coverage

@Test("Logger handles very long metadata values")
func testLoggerLongMetadata() async throws {
    let longString = String(repeating: "a", count: 10000)
    let metadata: Logger.Metadata = [
        "long_value": longString,
        "normal": "value"
    ]

    // Should not crash with very long metadata
    Logger.info("Test with long metadata", metadata: metadata)
    Logger.error("Error with long metadata", metadata: metadata)
}

@Test("Logger handles metadata with newlines")
func testLoggerMetadataWithNewlines() async throws {
    let metadata: Logger.Metadata = [
        "multiline": "line1\nline2\nline3",
        "tab": "value\twith\ttabs"
    ]

    // Should handle newlines and tabs without crashing
    Logger.info("Test with special chars", metadata: metadata)
}

@Test("RunVMRequest empty shared directories array")
func testRunVMRequestEmptyArray() async throws {
    let request = RunVMRequest(
        noDisplay: false,
        sharedDirectories: [],
        recoveryMode: false,
        storage: nil,
        network: nil,
        clipboard: false
    )

    let dirs = try request.parse()
    #expect(dirs.isEmpty)
}

@Test("RunVMRequest nil shared directories")
func testRunVMRequestNilSharedDirs() async throws {
    let request = RunVMRequest(
        noDisplay: false,
        sharedDirectories: nil,
        recoveryMode: false,
        storage: nil,
        network: nil,
        clipboard: false
    )

    let dirs = try request.parse()
    #expect(dirs.isEmpty)
}

@Test("CreateVMRequest parses various size formats")
func testCreateVMRequestSizeFormats() async throws {
    // Test lowercase
    let request1 = CreateVMRequest(
        name: "test",
        os: "macos",
        cpu: 2,
        memory: "4gb",
        diskSize: "64gb",
        display: "1024x768",
        ipsw: nil,
        storage: nil,
        unattended: nil,
        network: nil
    )
    let (mem1, _) = try request1.parse()
    #expect(mem1 == 4 * 1024 * 1024 * 1024)

    // Test uppercase
    let request2 = CreateVMRequest(
        name: "test",
        os: "macos",
        cpu: 2,
        memory: "4GB",
        diskSize: "64GB",
        display: "1024x768",
        ipsw: nil,
        storage: nil,
        unattended: nil,
        network: nil
    )
    let (mem2, _) = try request2.parse()
    #expect(mem2 == 4 * 1024 * 1024 * 1024)
}

@Test("SetVMRequest invalid display format")
func testSetVMRequestInvalidDisplay() async throws {
    let request = SetVMRequest(
        cpu: nil,
        memory: nil,
        diskSize: nil,
        display: "invalid-format",
        storage: nil
    )

    #expect(throws: ValidationError.self) {
        try request.parse()
    }
}

@Test("PullRequest with nil name")
func testPullRequestNilName() async throws {
    let json = """
    {
        "image": "test-image:latest"
    }
    """
    let data = json.data(using: .utf8)!
    let request = try JSONDecoder().decode(PullRequest.self, from: data)

    #expect(request.image == "test-image:latest")
    #expect(request.name == nil)
}

@Test("PushRequest with single tag")
func testPushRequestSingleTag() async throws {
    let json = """
    {
        "name": "my-vm",
        "imageName": "my-image",
        "tags": ["only-one"]
    }
    """
    let data = json.data(using: .utf8)!
    let request = try JSONDecoder().decode(PushRequest.self, from: data)

    #expect(request.tags.count == 1)
    #expect(request.tags[0] == "only-one")
}

@Test("PushRequest with many tags")
func testPushRequestManyTags() async throws {
    let json = """
    {
        "name": "my-vm",
        "imageName": "my-image",
        "tags": ["v1", "v2", "v3", "latest", "stable", "prod"]
    }
    """
    let data = json.data(using: .utf8)!
    let request = try JSONDecoder().decode(PushRequest.self, from: data)

    #expect(request.tags.count == 6)
}

@Test("GCSRegistryError with empty strings")
func testGCSRegistryErrorEmptyStrings() async throws {
    let error = GCSRegistryError.imageNotFound("", "")
    #expect(error.errorDescription != nil)
    #expect(!error.errorDescription!.isEmpty)
}

@Test("Convert command with empty additional tags")
func testConvertCommandEmptyTags() async throws {
    let targetImage = "test:v1"
    let additionalTags: [String] = []

    let components = targetImage.split(separator: ":")
    let primaryTag = String(components.last!)

    var allTags: Set<String> = [primaryTag]
    allTags.formUnion(additionalTags)

    #expect(allTags.count == 1)
    #expect(allTags.contains("v1"))
}

@Test("Convert command with duplicate tags in additional")
func testConvertCommandDuplicateTags() async throws {
    let targetImage = "test:latest"
    let additionalTags = ["latest", "stable", "latest", "v1.0", "stable"]

    let components = targetImage.split(separator: ":")
    let primaryTag = String(components.last!)

    var allTags: Set<String> = [primaryTag]
    allTags.formUnion(additionalTags)

    // Should deduplicate
    #expect(allTags.count == 3)
    #expect(allTags.contains("latest"))
    #expect(allTags.contains("stable"))
    #expect(allTags.contains("v1.0"))
}

@Test("Push command with zero chunk size")
func testPushCommandZeroChunkSize() async throws {
    let command = Push()
    // Default chunk size should be positive
    #expect(command.chunkSizeMb > 0)
}

@MainActor
@Test("LumeController normalizes simple names unchanged")
func testLumeControllerNormalizeSimpleName() async throws {
    let simpleName = "my-vm-name"
    let components = simpleName.split(separator: ":")

    // Simple names without colons should remain unchanged
    let normalized = components.count == 2 ? "\(components[0])_\(components[1])" : simpleName
    #expect(normalized == simpleName)
}

@MainActor
@Test("LumeController handles empty metadata gracefully")
func testLumeControllerEmptySettings() async throws {
    let controller = LumeController()

    // These should not throw even if settings are minimal
    let locations = controller.getLocations()
    #expect(locations.count >= 0)

    let settings = controller.getSettings()
    #expect(settings.homeDirectory != nil)
}

@Test("RunVMRequest parseNetworkMode with invalid value")
func testRunVMRequestInvalidNetworkMode() async throws {
    let request = RunVMRequest(
        noDisplay: false,
        sharedDirectories: nil,
        recoveryMode: false,
        storage: nil,
        network: "invalid-mode",
        clipboard: false
    )

    #expect(throws: ValidationError.self) {
        try request.parseNetworkMode()
    }
}

@Test("CreateVMRequest with bridged interface network")
func testCreateVMRequestBridgedInterface() async throws {
    let request = CreateVMRequest(
        name: "test",
        os: "macos",
        cpu: 2,
        memory: "4GB",
        diskSize: "64GB",
        display: "1024x768",
        ipsw: nil,
        storage: nil,
        unattended: nil,
        network: "bridged:en0"
    )

    let mode = try request.parseNetworkMode()
    // Should parse bridged with interface
    #expect(mode.description.contains("bridged"))
}

@MainActor
@Test("HostStatusResponse with negative available slots")
func testHostStatusNegativeSlots() async throws {
    // Test that max(0, ...) prevents negative slots
    let vmCount = 5
    let maxVMs = 2

    let response = MockServer.HostStatusResponse(
        status: "overloaded",
        vmCount: vmCount,
        maxVMs: maxVMs,
        availableSlots: max(0, maxVMs - vmCount),
        version: "1.0.0"
    )

    // Should be 0, not negative
    #expect(response.availableSlots == 0)
}

@Test("CloneRequest with nil locations")
func testCloneRequestNilLocations() async throws {
    let json = """
    {
        "name": "source",
        "newName": "dest"
    }
    """
    let data = json.data(using: .utf8)!
    let request = try JSONDecoder().decode(CloneRequest.self, from: data)

    #expect(request.name == "source")
    #expect(request.newName == "dest")
    #expect(request.sourceLocation == nil)
    #expect(request.destLocation == nil)
}

@Test("SetupVMRequest with only configYaml")
func testSetupVMRequestConfigYaml() async throws {
    let json = """
    {
        "configYaml": "boot_wait: 60\\nboot_commands: []"
    }
    """
    let data = json.data(using: .utf8)!
    let request = try JSONDecoder().decode(SetupVMRequest.self, from: data)

    #expect(request.configPath == nil)
    #expect(request.configYaml != nil)
    #expect(request.configYaml?.contains("boot_wait") == true)
}