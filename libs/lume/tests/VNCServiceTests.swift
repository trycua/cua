import Foundation
import Testing
@testable import lume

@Test("VNCService starts correctly")
func testVNCServiceStart() async throws {
    let tempDir = try createTempDirectory()
    defer { try? FileManager.default.removeItem(at: tempDir) }
    let vmDir = VMDirectory(Path(tempDir.path))
    let service = await MockVNCService(vmDirectory: vmDir)
    
    // Initial state
    let isRunning = await service.isRunning
    let url = await service.url
    #expect(!isRunning)
    #expect(url == nil)
    
    // Start service
    try await service.start(port: 5900, virtualMachine: nil)
    #expect(await service.isRunning)
    #expect(await service.url?.contains("5900") ?? false)
}

@Test("VNCService stops correctly")
func testVNCServiceStop() async throws {
    let tempDir = try createTempDirectory()
    defer { try? FileManager.default.removeItem(at: tempDir) }
    let vmDir = VMDirectory(Path(tempDir.path))
    let service = await MockVNCService(vmDirectory: vmDir)
    try await service.start(port: 5900, virtualMachine: nil)
    
    await service.stop()
    let isRunning = await service.isRunning
    let url = await service.url
    #expect(!isRunning)
    #expect(url == nil)
}

@Test("VNCService handles client operations")
func testVNCServiceClient() async throws {
    let tempDir = try createTempDirectory()
    defer { try? FileManager.default.removeItem(at: tempDir) }
    let vmDir = VMDirectory(Path(tempDir.path))
    let service = await MockVNCService(vmDirectory: vmDir)
    
    // Should fail when not started
    do {
        try await service.openClient(url: "vnc://localhost:5900")
        #expect(Bool(false), "Expected openClient to throw when not started")
    } catch VMError.vncNotConfigured {
        // Expected error
    } catch {
        #expect(Bool(false), "Expected vncNotConfigured error but got \(error)")
    }
    
    // Start and try client operations
    try await service.start(port: 5900, virtualMachine: nil)
    try await service.openClient(url: "vnc://localhost:5900")
    #expect(await service.clientOpenCount == 1)
    
    // Stop and verify client operations fail
    await service.stop()
    do {
        try await service.openClient(url: "vnc://localhost:5900")
        #expect(Bool(false), "Expected openClient to throw after stopping")
    } catch VMError.vncNotConfigured {
        // Expected error
    } catch {
        #expect(Bool(false), "Expected vncNotConfigured error but got \(error)")
    }
}

@Test("VNCService handles virtual machine attachment")
func testVNCServiceVMAttachment() async throws {
    let tempDir = try createTempDirectory()
    defer { try? FileManager.default.removeItem(at: tempDir) }
    let vmDir = VMDirectory(Path(tempDir.path))
    let service = await MockVNCService(vmDirectory: vmDir)
    let mockVM = "mock_vm"
    
    try await service.start(port: 5900, virtualMachine: mockVM)
    let attachedVM = await service.attachedVM
    #expect(attachedVM == mockVM)
}

private func createTempDirectory() throws -> URL {
    let tempDir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
    return tempDir
} 