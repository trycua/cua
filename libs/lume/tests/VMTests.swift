import Foundation
import Testing

@testable import lume

class MockProcessRunner: ProcessRunner {
    var runCalls: [(executable: String, arguments: [String])] = []

    func run(executable: String, arguments: [String]) throws {
        runCalls.append((executable, arguments))
    }
}

private struct VMOwnerRunLockProbe: RunLockProbe {
    let processIdentifiers: [pid_t]

    func lockHolderPIDs(ofFileAt path: String) -> [pid_t]? {
        processIdentifiers
    }
}

private func setupVMDirectory(_ tempDir: URL) throws -> VMDirectory {
    let vmDir = VMDirectory(Path(tempDir.path))

    // Create disk image file
    let diskPath = vmDir.diskPath
    let diskData = Data(repeating: 0, count: 1024 * 1024)  // 1MB mock disk
    try diskData.write(to: diskPath.url)

    // Create nvram file
    let nvramPath = vmDir.nvramPath
    let nvramData = Data(repeating: 0, count: 1024)  // 1KB mock nvram
    try nvramData.write(to: nvramPath.url)

    // Create initial config file
    var config = try VMConfig(
        os: "mock-os",
        cpuCount: 1,
        memorySize: 1024,
        diskSize: 1024,
        display: "1024x768"
    )
    config.setMacAddress("00:11:22:33:44:55")
    try vmDir.saveConfig(config)

    // Create .initialized file to mark VM as initialized
    let initializedPath = vmDir.dir.file(".initialized")
    try Data().write(to: initializedPath.url)

    return vmDir
}

@MainActor
@Test("VM initialization and configuration")
func testVMInitialization() async throws {
    let tempDir = try createTempDirectory()
    defer { try? FileManager.default.removeItem(at: tempDir) }
    let vmDir = try setupVMDirectory(tempDir)
    var config = try VMConfig(
        os: "mock-os",
        cpuCount: 1,
        memorySize: 1024,
        diskSize: 1024,
        display: "1024x768"
    )
    config.setMacAddress("00:11:22:33:44:55")  // Set MAC address to avoid nil
    let home = Home(fileManager: FileManager.default)
    let context = VMDirContext(dir: vmDir, config: config, home: home, storage: nil)

    let vm = MockVM(
        vmDirContext: context,
        virtualizationServiceFactory: { _ in MockVMVirtualizationService() },
        vncServiceFactory: { MockVNCService(vmDirectory: $0) }
    )

    // Test initial state
    let details = vm.details
    #expect(details.name == vmDir.name)
    #expect(details.os == "mock-os")
    #expect(details.status == "stopped")
    #expect(details.vncUrl == nil)
}

@MainActor
@Test("VM run and stop operations")
func testVMRunAndStop() async throws {
    let tempDir = try createTempDirectory()
    defer { try? FileManager.default.removeItem(at: tempDir) }
    let vmDir = try setupVMDirectory(tempDir)
    var config = try VMConfig(
        os: "mock-os",
        cpuCount: 2,
        memorySize: 2048,
        diskSize: 1024,
        display: "1024x768"
    )
    config.setMacAddress("00:11:22:33:44:55")
    let home = Home(fileManager: FileManager.default)
    let context = VMDirContext(dir: vmDir, config: config, home: home, storage: nil)

    let vm = MockVM(
        vmDirContext: context,
        virtualizationServiceFactory: { _ in MockVMVirtualizationService() },
        vncServiceFactory: { MockVNCService(vmDirectory: $0) }
    )

    // Test running VM
    let runTask = Task {
        try await vm.run(
            displayMode: .vnc, sharedDirectories: [], mount: nil as Path?, vncPort: 0,
            recoveryMode: false)
    }

    // Give the VM time to start
    try await Task.sleep(nanoseconds: UInt64(1e9))

    // Test stopping VM
    try await vm.stop()
    runTask.cancel()
}

@MainActor
@Test("Legacy VM stop does not signal an unrelated config reader")
func testLegacyVMStopRefusesUnverifiedConfigReaders() async throws {
    let tempDir = try createTempDirectory()
    defer { try? FileManager.default.removeItem(at: tempDir) }
    let vmDir = try setupVMDirectory(tempDir)
    let lockHandle = try FileHandle(forWritingTo: vmDir.configPath.url)
    #expect(flock(lockHandle.fileDescriptor, LOCK_EX | LOCK_NB) == 0)
    defer {
        flock(lockHandle.fileDescriptor, LOCK_UN)
        try? lockHandle.close()
    }

    let readerHandle = try FileHandle(forReadingFrom: vmDir.configPath.url)
    let unrelatedReader = Process()
    unrelatedReader.executableURL = URL(fileURLWithPath: "/bin/sleep")
    unrelatedReader.arguments = ["60"]
    unrelatedReader.standardInput = readerHandle
    try unrelatedReader.run()
    try readerHandle.close()
    defer {
        if unrelatedReader.isRunning {
            unrelatedReader.terminate()
        }
        unrelatedReader.waitUntilExit()
    }

    let context = VMDirContext(
        dir: vmDir,
        config: try vmDir.loadConfig(),
        home: Home(fileManager: FileManager.default),
        storage: nil)
    let vm = MockVM(
        vmDirContext: context,
        virtualizationServiceFactory: { _ in MockVMVirtualizationService() },
        vncServiceFactory: { MockVNCService(vmDirectory: $0) }
    )

    do {
        try await vm.stop()
        Issue.record("Expected stop to reject the unverified legacy VM")
    } catch VMError.unverifiedProcessOwner(let name) {
        #expect(name == vmDir.name)
    } catch {
        Issue.record("Unexpected stop error: \(error)")
    }
    #expect(unrelatedReader.isRunning)
}

@Test("VM owner registry validates the current process and removes its record")
func testVMOwnerRegistryLifecycle() throws {
    let tempDir = try createTempDirectory()
    defer { try? FileManager.default.removeItem(at: tempDir) }
    let vmDirectory = VMDirectory(Path(tempDir.path))

    let owner = try VMProcessOwnerRegistry.register(vmDirectory: vmDirectory)
    #expect(VMProcessOwnerRegistry.validatedOwner(for: vmDirectory) == owner)
    #expect(owner.processIdentifier == getpid())

    VMProcessOwnerRegistry.unregister(owner, vmDirectory: vmDirectory)
    #expect(!VMProcessOwnerRegistry.ownerRecordExists(for: vmDirectory))
}

@Test("VM owner registry requires the recorded process to hold the run lock")
func testVMOwnerRegistryRequiresRunLockOwnership() throws {
    let tempDir = try createTempDirectory()
    defer { try? FileManager.default.removeItem(at: tempDir) }
    let vmDirectory = VMDirectory(Path(tempDir.path))
    let owner = try VMProcessOwnerRegistry.register(vmDirectory: vmDirectory)

    let ownerProbe = VMOwnerRunLockProbe(processIdentifiers: [owner.processIdentifier])
    #expect(
        VMProcessOwnerRegistry.validatedLockOwner(for: vmDirectory, using: ownerProbe) == owner)

    let unrelatedReaderProbe = VMOwnerRunLockProbe(
        processIdentifiers: [owner.processIdentifier + 1])
    #expect(
        VMProcessOwnerRegistry.validatedLockOwner(
            for: vmDirectory, using: unrelatedReaderProbe) == nil)
}

@Test("VM owner registry rejects a reused process identifier")
func testVMOwnerRegistryRejectsReusedProcessIdentifier() throws {
    let tempDir = try createTempDirectory()
    defer { try? FileManager.default.removeItem(at: tempDir) }
    let vmDirectory = VMDirectory(Path(tempDir.path))
    let owner = try VMProcessOwnerRegistry.register(vmDirectory: vmDirectory)
    let staleOwner = VMProcessOwner(
        processIdentifier: owner.processIdentifier,
        startTimeSeconds: owner.startTimeSeconds + 1,
        startTimeMicroseconds: owner.startTimeMicroseconds)
    let ownerFile = vmDirectory.dir.file(".vm-process-owner.json").url
    try JSONEncoder().encode(staleOwner).write(to: ownerFile, options: .atomic)

    #expect(VMProcessOwnerRegistry.validatedOwner(for: vmDirectory) == nil)
    #expect(VMProcessOwnerRegistry.ownerRecordExists(for: vmDirectory))
}

@MainActor
@Test("VM configuration updates")
func testVMConfigurationUpdates() async throws {
    let tempDir = try createTempDirectory()
    defer { try? FileManager.default.removeItem(at: tempDir) }
    let vmDir = try setupVMDirectory(tempDir)
    var config = try VMConfig(
        os: "mock-os",
        cpuCount: 1,
        memorySize: 1024,
        diskSize: 1024,
        display: "1024x768"
    )
    config.setMacAddress("00:11:22:33:44:55")
    let home = Home(fileManager: FileManager.default)
    let context = VMDirContext(dir: vmDir, config: config, home: home, storage: nil)

    let vm = MockVM(
        vmDirContext: context,
        virtualizationServiceFactory: { _ in MockVMVirtualizationService() },
        vncServiceFactory: { MockVNCService(vmDirectory: $0) }
    )

    // Test CPU count update
    try vm.setCpuCount(4)
    #expect(vm.vmDirContext.config.cpuCount == 4)

    // Test memory size update
    try vm.setMemorySize(4096)
    #expect(vm.vmDirContext.config.memorySize == 4096)

    // Test MAC address update
    try vm.setMacAddress("00:11:22:33:44:66")
    #expect(vm.vmDirContext.config.macAddress == "00:11:22:33:44:66")
}

@MainActor
@Test("VM virtualization context forwards additional disks")
func testVMVirtualizationContextForwardsAdditionalDisks() throws {
    let tempDir = try createTempDirectory()
    defer { try? FileManager.default.removeItem(at: tempDir) }
    let vmDir = try setupVMDirectory(tempDir)
    let additionalDiskURLs = [
        tempDir.appendingPathComponent("scratch.img"),
        tempDir.appendingPathComponent("cache.img"),
    ]
    for url in additionalDiskURLs {
        try Data(repeating: 0, count: 1024).write(to: url)
    }

    var config = try VMConfig(
        os: "mock-os",
        cpuCount: 1,
        memorySize: 1024,
        diskSize: 1024,
        display: "1024x768"
    )
    config.setMacAddress("00:11:22:33:44:55")
    let context = VMDirContext(
        dir: vmDir,
        config: config,
        home: Home(fileManager: FileManager.default),
        storage: nil
    )
    let vm = MockVM(
        vmDirContext: context,
        virtualizationServiceFactory: { _ in MockVMVirtualizationService() },
        vncServiceFactory: { MockVNCService(vmDirectory: $0) }
    )
    let additionalDisks = additionalDiskURLs.map(Path.init)

    let serviceContext = try vm.createVMVirtualizationServiceContext(
        cpuCount: 1,
        memorySize: 1024,
        display: "1024x768",
        additionalDiskPaths: additionalDisks
    )

    #expect(serviceContext.additionalDiskPaths?.map(\.path) == additionalDisks.map(\.path))
}

@MainActor
@Test("VM setup process")
func testVMSetup() async throws {
    let tempDir = try createTempDirectory()
    defer { try? FileManager.default.removeItem(at: tempDir) }
    let vmDir = try setupVMDirectory(tempDir)
    var config = try VMConfig(
        os: "mock-os",
        cpuCount: 1,
        memorySize: 1024,
        diskSize: 1024,
        display: "1024x768"
    )
    config.setMacAddress("00:11:22:33:44:55")
    let home = Home(fileManager: FileManager.default)
    let context = VMDirContext(dir: vmDir, config: config, home: home, storage: nil)

    let vm = MockVM(
        vmDirContext: context,
        virtualizationServiceFactory: { _ in MockVMVirtualizationService() },
        vncServiceFactory: { MockVNCService(vmDirectory: $0) }
    )

    let expectedDiskSize: UInt64 = 64 * 1024 * 1024 * 1024  // 64 GB

    try await vm.setup(
        ipswPath: "/path/to/mock.ipsw",
        cpuCount: 2,
        memorySize: 2048,
        diskSize: expectedDiskSize,
        display: "1024x768"
    )

    #expect(vm.vmDirContext.config.cpuCount == 2)
    #expect(vm.vmDirContext.config.memorySize == 2048)
    let actualDiskSize = vm.vmDirContext.config.diskSize ?? 0
    #expect(
        actualDiskSize == expectedDiskSize,
        "Expected disk size \(expectedDiskSize), but got \(actualDiskSize)")
    #expect(vm.vmDirContext.config.macAddress == "00:11:22:33:44:55")
}

private func createTempDirectory() throws -> URL {
    let tempDir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
    return tempDir
}
