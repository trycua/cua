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

private func createTempDirectory() throws -> URL {
    let tempDir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
    return tempDir
}
