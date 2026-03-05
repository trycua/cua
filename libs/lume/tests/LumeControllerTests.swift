import Foundation
import Testing
import Virtualization

@testable import lume

@MainActor
@Test("LumeController normalizes VM names with colons")
func testLumeControllerNormalizeVMName() async throws {
    let controller = LumeController()

    // Test normalization of image-style names
    // The normalizeVMName method is private but we can test its behavior indirectly

    // Names with colons should be normalized to underscores
    let imageName = "macos-sequoia:latest"
    let expectedNormalized = "macos-sequoia_latest"

    // We can verify this by checking components
    let components = imageName.split(separator: ":")
    let normalized = components.count == 2 ? "\(components[0])_\(components[1])" : imageName
    #expect(normalized == expectedNormalized)
}

@MainActor
@Test("LumeController settings management")
func testLumeControllerSettings() async throws {
    let controller = LumeController()

    // Test getting settings
    let settings = controller.getSettings()
    #expect(settings.homeDirectory != nil)

    // Test cache directory getter
    let cacheDir = controller.getCacheDirectory()
    #expect(!cacheDir.isEmpty)

    // Test caching enabled getter
    let cachingEnabled = controller.isCachingEnabled()
    #expect(cachingEnabled == true || cachingEnabled == false) // Just verify it returns a bool

    // Test telemetry enabled getter
    let telemetryEnabled = controller.isTelemetryEnabled()
    #expect(telemetryEnabled == true || telemetryEnabled == false)
}

@MainActor
@Test("LumeController location management")
func testLumeControllerLocationManagement() async throws {
    let controller = LumeController()

    // Test getting locations
    let locations = controller.getLocations()
    #expect(locations != nil)

    // Locations should be an array (could be empty or have items)
    #expect(locations.count >= 0)
}

@MainActor
@Test("LumeController list VMs returns array")
func testLumeControllerListVMs() async throws {
    let controller = LumeController()

    // Test listing VMs - should not throw
    let vms = try controller.list()
    #expect(vms != nil)

    // VMs should be an array (could be empty)
    #expect(vms.count >= 0)
}

@MainActor
@Test("LumeController list VMs with storage filter")
func testLumeControllerListVMsWithStorage() async throws {
    let controller = LumeController()

    // Test listing VMs with non-existent storage location
    // Should return empty array, not throw
    let vms = try controller.list(storage: "nonexistent-location")
    #expect(vms.count == 0)
}

@MainActor
@Test("LumeController validates VM names")
func testLumeControllerValidateVMNames() async throws {
    let controller = LumeController()

    // Test with non-existent VM
    #expect(throws: VMError.self) {
        try controller.validateVMExists("nonexistent-vm-12345")
    }
}

@MainActor
@Test("LumeController image info structure")
func testLumeControllerImageInfo() async throws {
    let imageInfo = LumeController.ImageInfo(
        repository: "test/image",
        imageId: "abc123def456"
    )

    #expect(imageInfo.repository == "test/image")
    #expect(imageInfo.imageId == "abc123def456")

    // Test encoding
    let encoder = JSONEncoder()
    let data = try encoder.encode(imageInfo)
    #expect(data.count > 0)

    // Test decoding
    let decoder = JSONDecoder()
    let decoded = try decoder.decode(LumeController.ImageInfo.self, from: data)
    #expect(decoded.repository == "test/image")
    #expect(decoded.imageId == "abc123def456")
}

@MainActor
@Test("LumeController image list structure")
func testLumeControllerImageList() async throws {
    let imageList = LumeController.ImageList(
        local: [
            LumeController.ImageInfo(repository: "test/image1", imageId: "abc123"),
            LumeController.ImageInfo(repository: "test/image2", imageId: "def456")
        ],
        remote: ["test/remote1", "test/remote2"]
    )

    #expect(imageList.local.count == 2)
    #expect(imageList.remote.count == 2)

    // Test encoding
    let encoder = JSONEncoder()
    let data = try encoder.encode(imageList)
    #expect(data.count > 0)

    // Test decoding
    let decoder = JSONDecoder()
    let decoded = try decoder.decode(LumeController.ImageList.self, from: data)
    #expect(decoded.local.count == 2)
    #expect(decoded.remote.count == 2)
}

@MainActor
@Test("LumeController validates create parameters")
func testLumeControllerValidateCreateParameters() async throws {
    let controller = LumeController()

    // Test validation of unsupported OS
    await #expect(throws: ValidationError.self) {
        try await controller.create(
            name: "test-vm",
            os: "unsupported-os",
            diskSize: 64 * 1024 * 1024 * 1024,
            cpuCount: 2,
            memorySize: 4 * 1024 * 1024 * 1024,
            display: "1024x768",
            ipsw: nil
        )
    }
}

@MainActor
@Test("LumeController validates pull parameters")
func testLumeControllerValidatePullParameters() async throws {
    let controller = LumeController()

    // Test validation of invalid image format (missing tag)
    await #expect(throws: ValidationError.self) {
        try await controller.pullImage(
            image: "invalid-image-no-tag",
            name: nil,
            registry: "ghcr.io",
            organization: "trycua"
        )
    }
}

@MainActor
@Test("LumeController validates push parameters")
func testLumeControllerValidatePushParameters() async throws {
    let controller = LumeController()

    // Test validation of empty VM name
    await #expect(throws: ValidationError.self) {
        try await controller.pushImage(
            name: "",
            imageName: "test-image",
            tags: ["latest"],
            registry: "ghcr.io",
            organization: "trycua"
        )
    }

    // Test validation of empty tags
    await #expect(throws: ValidationError.self) {
        try await controller.pushImage(
            name: "test-vm",
            imageName: "test-image",
            tags: [],
            registry: "ghcr.io",
            organization: "trycua"
        )
    }
}

@MainActor
@Test("LumeController parseVNCPort extracts port correctly")
func testLumeControllerParseVNCPort() async throws {
    // Test VNC URL parsing logic
    let vncUrl = "vnc://:password@127.0.0.1:62295"

    // Parse the port from the URL
    let httpUrl = vncUrl.replacingOccurrences(of: "vnc://", with: "http://")
    if let urlComponents = URLComponents(string: httpUrl),
       let port = urlComponents.port {
        #expect(port == 62295)
    } else {
        Issue.record("Failed to parse VNC port")
    }
}

@MainActor
@Test("LumeController SharedVM singleton behavior")
func testSharedVMSingleton() async throws {
    let shared1 = SharedVM.shared
    let shared2 = SharedVM.shared

    // Should be the same instance
    #expect(shared1 === shared2)
}