import Foundation
import Testing
import Virtualization
@testable import lume

@MainActor
@Test("VMVirtualizationService starts correctly")
func testVMVirtualizationServiceStart() async throws {
    let service = MockVMVirtualizationService()
    
    // Initial state
    #expect(await service.state == .stopped)
    #expect(await service.startCallCount == 0)
    
    // Start service
    try await service.start()
    #expect(await service.state == .running)
    #expect(await service.startCallCount == 1)
}

@MainActor
@Test("VMVirtualizationService stops correctly")
func testVMVirtualizationServiceStop() async throws {
    let service = MockVMVirtualizationService()
    
    // Start then stop
    try await service.start()
    try await service.stop()
    
    #expect(await service.state == .stopped)
    #expect(await service.stopCallCount == 1)
}

@MainActor
@Test("VMVirtualizationService handles pause and resume")
func testVMVirtualizationServicePauseResume() async throws {
    let service = MockVMVirtualizationService()
    
    // Start and pause
    try await service.start()
    try await service.pause()
    #expect(await service.state == .paused)
    #expect(await service.pauseCallCount == 1)
    
    // Resume
    try await service.resume()
    #expect(await service.state == .running)
    #expect(await service.resumeCallCount == 1)
}

@MainActor
@Test("VMVirtualizationService handles operation failures")
func testVMVirtualizationServiceFailures() async throws {
    let service = MockVMVirtualizationService()
    await service.configure(shouldFail: true)
    
    // Test start failure
    do {
        try await service.start()
        #expect(Bool(false), "Expected start to throw")
    } catch let error as VMError {
        switch error {
        case .internalError(let message):
            #expect(message == "Mock operation failed")
        default:
            #expect(Bool(false), "Unexpected error type: \(error)")
        }
    }
    
    #expect(await service.state == .stopped)
    #expect(await service.startCallCount == 1)
}

@MainActor
@Test("Shared directories with one tag use a disambiguated multiple-directory share")
func testSharedDirectoryGrouping() throws {
    let tag = VZVirtioFileSystemDeviceConfiguration.macOSGuestAutomountTag
    let sharedDirectories = [
        SharedDirectory(hostPath: "/tmp/first/Shared", tag: tag, readOnly: false),
        SharedDirectory(hostPath: "/tmp/second/Shared", tag: tag, readOnly: true),
    ]

    let devices = BaseVirtualizationService.createDirectorySharingDevices(
        sharedDirectories: sharedDirectories
    )
    let device = try #require(devices.first as? VZVirtioFileSystemDeviceConfiguration)
    let share = try #require(device.share as? VZMultipleDirectoryShare)

    #expect(devices.count == 1)
    #expect(device.tag == tag)
    let names = Swift.Set<String>(share.directories.keys)
    #expect(names == Swift.Set(["Shared", "Shared (2)"]))
    #expect(share.directories["Shared"]?.isReadOnly == false)
    #expect(share.directories["Shared (2)"]?.isReadOnly == true)
}