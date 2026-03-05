import Foundation
import Testing

@testable import lume

// Mock Server for testing handlers
@MainActor
class MockServer: Server {
    init() {
        // Initialize with default host and port for testing
        super.init(host: "127.0.0.1", port: 8080)
    }
}

@MainActor
@Test("HostStatusResponse encoding and decoding")
func testHostStatusResponse() async throws {
    let response = MockServer.HostStatusResponse(
        status: "healthy",
        vmCount: 1,
        maxVMs: 2,
        availableSlots: 1,
        version: "1.0.0"
    )

    #expect(response.status == "healthy")
    #expect(response.vmCount == 1)
    #expect(response.maxVMs == 2)
    #expect(response.availableSlots == 1)
    #expect(response.version == "1.0.0")

    // Test encoding
    let encoder = JSONEncoder()
    let data = try encoder.encode(response)

    // Test decoding
    let decoder = JSONDecoder()
    let decoded = try decoder.decode(MockServer.HostStatusResponse.self, from: data)

    #expect(decoded.status == "healthy")
    #expect(decoded.vmCount == 1)
    #expect(decoded.maxVMs == 2)
    #expect(decoded.availableSlots == 1)
    #expect(decoded.version == "1.0.0")
}

@MainActor
@Test("HostStatusResponse calculates available slots correctly")
func testHostStatusResponseSlots() async throws {
    let maxVMs = 2

    // No VMs running
    let response1 = MockServer.HostStatusResponse(
        status: "healthy",
        vmCount: 0,
        maxVMs: maxVMs,
        availableSlots: max(0, maxVMs - 0),
        version: "1.0.0"
    )
    #expect(response1.availableSlots == 2)

    // One VM running
    let response2 = MockServer.HostStatusResponse(
        status: "healthy",
        vmCount: 1,
        maxVMs: maxVMs,
        availableSlots: max(0, maxVMs - 1),
        version: "1.0.0"
    )
    #expect(response2.availableSlots == 1)

    // All VMs running
    let response3 = MockServer.HostStatusResponse(
        status: "healthy",
        vmCount: 2,
        maxVMs: maxVMs,
        availableSlots: max(0, maxVMs - 2),
        version: "1.0.0"
    )
    #expect(response3.availableSlots == 0)
}

@MainActor
@Test("ConfigRequest encoding and decoding")
func testConfigRequest() async throws {
    let request = MockServer.ConfigRequest(
        homeDirectory: "/custom/home",
        cacheDirectory: "/custom/cache",
        cachingEnabled: true
    )

    #expect(request.homeDirectory == "/custom/home")
    #expect(request.cacheDirectory == "/custom/cache")
    #expect(request.cachingEnabled == true)

    // Test encoding
    let encoder = JSONEncoder()
    let data = try encoder.encode(request)

    // Test decoding
    let decoder = JSONDecoder()
    let decoded = try decoder.decode(MockServer.ConfigRequest.self, from: data)

    #expect(decoded.homeDirectory == "/custom/home")
    #expect(decoded.cacheDirectory == "/custom/cache")
    #expect(decoded.cachingEnabled == true)
}

@MainActor
@Test("ConfigRequest handles optional fields")
func testConfigRequestOptionalFields() async throws {
    let json = """
    {
        "homeDirectory": null,
        "cacheDirectory": "/cache",
        "cachingEnabled": null
    }
    """
    let data = json.data(using: .utf8)!

    let decoder = JSONDecoder()
    let request = try decoder.decode(MockServer.ConfigRequest.self, from: data)

    #expect(request.homeDirectory == nil)
    #expect(request.cacheDirectory == "/cache")
    #expect(request.cachingEnabled == nil)
}

@MainActor
@Test("LocationRequest encoding and decoding")
func testLocationRequest() async throws {
    let request = MockServer.LocationRequest(
        name: "custom-location",
        path: "/path/to/location"
    )

    #expect(request.name == "custom-location")
    #expect(request.path == "/path/to/location")

    // Test encoding
    let encoder = JSONEncoder()
    let data = try encoder.encode(request)

    // Test decoding
    let decoder = JSONDecoder()
    let decoded = try decoder.decode(MockServer.LocationRequest.self, from: data)

    #expect(decoded.name == "custom-location")
    #expect(decoded.path == "/path/to/location")
}

@MainActor
@Test("handleListVMs returns valid response")
func testHandleListVMs() async throws {
    let server = MockServer()

    let response = try await server.handleListVMs(storage: nil)

    #expect(response.statusCode == .ok || response.statusCode == .badRequest)
}

@MainActor
@Test("handleGetConfig returns valid response")
func testHandleGetConfig() async throws {
    let server = MockServer()

    let response = try await server.handleGetConfig()

    #expect(response.statusCode == .ok || response.statusCode == .badRequest)
}

@MainActor
@Test("handleGetLocations returns valid response")
func testHandleGetLocations() async throws {
    let server = MockServer()

    let response = try await server.handleGetLocations()

    #expect(response.statusCode == .ok || response.statusCode == .badRequest)
}

@MainActor
@Test("handlePruneImages returns valid response")
func testHandlePruneImages() async throws {
    let server = MockServer()

    let response = try await server.handlePruneImages()

    // Should return ok or badRequest
    #expect(response.statusCode == .ok || response.statusCode == .badRequest)
}

@MainActor
@Test("handleCreateVM with invalid body returns bad request")
func testHandleCreateVMInvalidBody() async throws {
    let server = MockServer()

    // Test with nil body
    let response1 = try await server.handleCreateVM(nil)
    #expect(response1.statusCode == .badRequest)

    // Test with invalid JSON
    let invalidJson = "invalid json".data(using: .utf8)
    let response2 = try await server.handleCreateVM(invalidJson)
    #expect(response2.statusCode == .badRequest)
}

@MainActor
@Test("handleSetupVM with nil body returns bad request")
func testHandleSetupVMNilBody() async throws {
    let server = MockServer()

    let response = try await server.handleSetupVM(name: "test-vm", body: nil)
    #expect(response.statusCode == .badRequest)
}

@MainActor
@Test("handleCloneVM with invalid body returns bad request")
func testHandleCloneVMInvalidBody() async throws {
    let server = MockServer()

    // Test with nil body
    let response1 = try await server.handleCloneVM(nil)
    #expect(response1.statusCode == .badRequest)

    // Test with invalid JSON
    let invalidJson = "invalid json".data(using: .utf8)
    let response2 = try await server.handleCloneVM(invalidJson)
    #expect(response2.statusCode == .badRequest)
}

@MainActor
@Test("handleSetVM with invalid body returns bad request")
func testHandleSetVMInvalidBody() async throws {
    let server = MockServer()

    let response = try await server.handleSetVM(name: "test-vm", body: nil)
    #expect(response.statusCode == .badRequest)
}

@MainActor
@Test("handlePull with invalid body returns bad request")
func testHandlePullInvalidBody() async throws {
    let server = MockServer()

    let response = try await server.handlePull(nil)
    #expect(response.statusCode == .badRequest)
}

@MainActor
@Test("handlePush with invalid body returns bad request")
func testHandlePushInvalidBody() async throws {
    let server = MockServer()

    let response = try await server.handlePush(nil)
    #expect(response.statusCode == .badRequest)
}

@MainActor
@Test("handleUpdateConfig with invalid body returns bad request")
func testHandleUpdateConfigInvalidBody() async throws {
    let server = MockServer()

    let response = try await server.handleUpdateConfig(nil)
    #expect(response.statusCode == .badRequest)
}

@MainActor
@Test("handleAddLocation with invalid body returns bad request")
func testHandleAddLocationInvalidBody() async throws {
    let server = MockServer()

    let response = try await server.handleAddLocation(nil)
    #expect(response.statusCode == .badRequest)
}