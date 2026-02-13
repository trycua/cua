import Foundation

@testable import lume

@MainActor
class MockVM: VM {
    private var mockIsRunning = false

    override func getOSType() -> String {
        return "mock-os"
    }

    override func setup(
        ipswPath: String, cpuCount: Int, memorySize: UInt64, diskSize: UInt64, display: String
    ) async throws {
        // Mock setup implementation
        vmDirContext.config.setCpuCount(cpuCount)
        vmDirContext.config.setMemorySize(memorySize)
        vmDirContext.config.setDiskSize(diskSize)
        vmDirContext.config.setMacAddress("00:11:22:33:44:55")
        try vmDirContext.saveConfig()
    }

    override func run(
        noDisplay: Bool, sharedDirectories: [SharedDirectory], mount: Path?, vncPort: Int = 0,
        recoveryMode: Bool = false, usbMassStoragePaths: [Path]? = nil
    ) async throws {
        mockIsRunning = true
        try await super.run(
            noDisplay: noDisplay, sharedDirectories: sharedDirectories, mount: mount,
            vncPort: vncPort, recoveryMode: recoveryMode,
            usbMassStoragePaths: usbMassStoragePaths
        )
    }

    override func stop() async throws {
        mockIsRunning = false
        try await super.stop()
    }
}
