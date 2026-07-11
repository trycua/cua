import Foundation
import Testing

@testable import lume

private enum TestCreateError: Error {
    case setupFailed
}

@MainActor
private final class FailingVM: MockVM {
    override func setup(
        ipswPath: String,
        cpuCount: Int,
        memorySize: UInt64,
        diskSize: UInt64,
        display: String
    ) async throws {
        throw TestCreateError.setupFailed
    }
}

@MainActor
private final class FailingVMFactory: VMFactory {
    func createVM(
        vmDirContext: VMDirContext,
        imageLoader: ImageLoader?
    ) throws -> VM {
        FailingVM(
            vmDirContext: vmDirContext,
            virtualizationServiceFactory: { _ in MockVMVirtualizationService() },
            vncServiceFactory: { MockVNCService(vmDirectory: $0) }
        )
    }
}

@MainActor
private final class TestVMFactory: VMFactory {
    func createVM(
        vmDirContext: VMDirContext,
        imageLoader: ImageLoader?
    ) throws -> VM {
        MockVM(
            vmDirContext: vmDirContext,
            virtualizationServiceFactory: { _ in MockVMVirtualizationService() },
            vncServiceFactory: { MockVNCService(vmDirectory: $0) }
        )
    }
}

@MainActor
@Test("create cleans up pre-created VM directory when setup fails")
func testCreateCleansUpPrecreatedDirectoryOnFailure() async throws {
    let tempConfigDir = try createTempDirectory()
    let tempHomeDir = try createTempDirectory()
    let tempStorageDir = try createTempDirectory()

    defer {
        try? FileManager.default.removeItem(at: tempConfigDir)
        try? FileManager.default.removeItem(at: tempHomeDir)
        try? FileManager.default.removeItem(at: tempStorageDir)
    }

    let previousXDGConfigHome = ProcessInfo.processInfo.environment["XDG_CONFIG_HOME"]
    setenv("XDG_CONFIG_HOME", tempConfigDir.path, 1)
    defer {
        if let previousXDGConfigHome {
            setenv("XDG_CONFIG_HOME", previousXDGConfigHome, 1)
        } else {
            unsetenv("XDG_CONFIG_HOME")
        }
    }

    let settingsManager = SettingsManager(fileManager: .default)
    try settingsManager.setHomeDirectory(path: tempHomeDir.path)

    let controller = LumeController(
        home: Home(settingsManager: settingsManager, fileManager: .default),
        vmFactory: FailingVMFactory()
    )
    let orphanVMDir = VMDirectory(Path(tempStorageDir.path).directory("orphan-vm"))

    do {
        try await controller.create(
            name: "orphan-vm",
            os: "linux",
            diskSize: 64 * 1024 * 1024,
            cpuCount: 1,
            memorySize: 1024 * 1024 * 1024,
            display: "1024x768",
            ipsw: nil,
            storage: tempStorageDir.path
        )
        #expect(Bool(false), "Expected create to fail with the injected VM factory")
    } catch TestCreateError.setupFailed {
        // Expected failure from the injected VM factory.
    } catch {
        #expect(Bool(false), "Expected setupFailed error but got \(error)")
    }

    #expect(!orphanVMDir.exists())
}

@MainActor
@Test("clone preserves paired disk and auxiliary storage")
func testClonePreservesPairedBootPolicyState() throws {
    let tempConfigDir = try createTempDirectory()
    let tempHomeDir = try createTempDirectory()

    defer {
        try? FileManager.default.removeItem(at: tempConfigDir)
        try? FileManager.default.removeItem(at: tempHomeDir)
    }

    let previousXDGConfigHome = ProcessInfo.processInfo.environment["XDG_CONFIG_HOME"]
    setenv("XDG_CONFIG_HOME", tempConfigDir.path, 1)
    defer {
        if let previousXDGConfigHome {
            setenv("XDG_CONFIG_HOME", previousXDGConfigHome, 1)
        } else {
            unsetenv("XDG_CONFIG_HOME")
        }
    }

    let settingsManager = SettingsManager(fileManager: .default)
    try settingsManager.setHomeDirectory(path: tempHomeDir.path)
    let home = Home(settingsManager: settingsManager, fileManager: .default)
    let controller = LumeController(home: home, vmFactory: TestVMFactory())

    let sourceDir = try home.getVMDirectory("sip-off-seed")
    try FileManager.default.createDirectory(
        at: sourceDir.dir.url,
        withIntermediateDirectories: true
    )

    let diskData = Data(repeating: 0xA5, count: 1024 * 1024)
    let nvramData = Data((0..<4096).map { UInt8($0 % 251) })
    try diskData.write(to: sourceDir.diskPath.url)
    try nvramData.write(to: sourceDir.nvramPath.url)

    let sourceMachineIdentifier = Data(repeating: 0x11, count: 32)
    let sourceMacAddress = "02:00:00:00:00:01"
    let sourceConfig = try VMConfig(
        os: "macOS",
        cpuCount: 4,
        memorySize: 8 * 1024 * 1024 * 1024,
        diskSize: UInt64(diskData.count),
        macAddress: sourceMacAddress,
        display: "1024x768",
        hardwareModel: Data(repeating: 0x22, count: 32),
        machineIdentifier: sourceMachineIdentifier
    )
    try sourceDir.saveConfig(sourceConfig)

    try controller.clone(name: "sip-off-seed", newName: "worker-001")

    let cloneDir = try home.getVMDirectory("worker-001")
    let cloneConfig = try cloneDir.loadConfig()

    #expect(try Data(contentsOf: cloneDir.diskPath.url) == diskData)
    #expect(try Data(contentsOf: cloneDir.nvramPath.url) == nvramData)
    #expect(cloneConfig.machineIdentifier != sourceMachineIdentifier)
    #expect(cloneConfig.macAddress != sourceMacAddress)
}

private func createTempDirectory() throws -> URL {
    let tempDir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
    return tempDir
}
