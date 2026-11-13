import Foundation
import Testing

@testable import lume

private struct FixedSpaceProbe: HostVolumeSpaceProbe {
    /// Free bytes by directory prefix: a file lives on the volume of the
    /// longest prefix that contains it.
    let volumes: [String: UInt64]

    func volume(of url: URL) -> (volume: URL, availableBytes: UInt64)? {
        let path = url.path(percentEncoded: false)
        guard let prefix = volumes.keys.filter({ path.hasPrefix($0) }).max(by: { $0.count < $1.count })
        else { return nil }
        return (URL(filePath: prefix, directoryHint: .isDirectory), volumes[prefix]!)
    }
}

@MainActor
private final class RecordingVMFactory: VMFactory {
    private(set) var services: [MockVMVirtualizationService] = []

    func createVM(vmDirContext: VMDirContext, imageLoader: ImageLoader?) throws -> VM {
        MockVM(
            vmDirContext: vmDirContext,
            virtualizationServiceFactory: { [unowned self] _ in
                let service = MockVMVirtualizationService()
                self.services.append(service)
                return service
            },
            vncServiceFactory: { MockVNCService(vmDirectory: $0) }
        )
    }
}

private let gib: UInt64 = 1024 * 1024 * 1024
private let mib: UInt64 = 1024 * 1024

/// A sparse file of `logicalBytes`: it takes almost no host blocks until written.
private func makeSparseDisk(at url: URL, logicalBytes: UInt64) throws {
    FileManager.default.createFile(atPath: url.path, contents: nil)
    let handle = try FileHandle(forWritingTo: url)
    defer { try? handle.close() }
    try handle.truncate(atOffset: logicalBytes)
}

private func makeTempDirectory() throws -> URL {
    let url = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    return url
}

@Test("A sparse disk needs the reserve free on its volume, not its allocated size")
func sparseDiskNeedsReserve() throws {
    let dir = try makeTempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let disk = dir.appendingPathComponent("disk.img")
    try makeSparseDisk(at: disk, logicalBytes: 50 * gib)

    let tight = FixedSpaceProbe(volumes: [dir.path: 115 * mib])
    #expect(
        HostDiskSpace.shortfall(writableDisks: [Path(disk)], probe: tight)
            == HostDiskSpace.Shortfall(
                volume: URL(filePath: dir.path, directoryHint: .isDirectory).path(percentEncoded: false),
                requiredBytes: HostDiskSpace.reserveBytes,
                availableBytes: 115 * mib))

    let roomy = FixedSpaceProbe(volumes: [dir.path: HostDiskSpace.reserveBytes])
    #expect(HostDiskSpace.shortfall(writableDisks: [Path(disk)], probe: roomy) == nil)
}

@Test("A disk smaller than the reserve needs only its own size free")
func smallDiskNeedsItsSize() throws {
    let dir = try makeTempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let disk = dir.appendingPathComponent("disk.img")
    try makeSparseDisk(at: disk, logicalBytes: 64 * mib)

    let exact = FixedSpaceProbe(volumes: [dir.path: 64 * mib])
    #expect(HostDiskSpace.shortfall(writableDisks: [Path(disk)], probe: exact) == nil)

    let short = FixedSpaceProbe(volumes: [dir.path: 64 * mib - 1])
    #expect(
        HostDiskSpace.shortfall(writableDisks: [Path(disk)], probe: short)?.requiredBytes
            == 64 * mib)
}

@Test("Disks on one volume share its free space; each volume is checked on its own")
func disksArePooledPerVolume() throws {
    let root = try makeTempDirectory()
    defer { try? FileManager.default.removeItem(at: root) }
    let volumeA = root.appendingPathComponent("a")
    let volumeB = root.appendingPathComponent("b")
    for dir in [volumeA, volumeB] {
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    }
    let first = volumeA.appendingPathComponent("disk.img")
    let second = volumeA.appendingPathComponent("data.img")
    let other = volumeB.appendingPathComponent("data.img")
    for disk in [first, second, other] {
        try makeSparseDisk(at: disk, logicalBytes: 100 * mib)
    }

    let probe = FixedSpaceProbe(volumes: [volumeA.path: 150 * mib, volumeB.path: 150 * mib])
    // One disk per volume fits.
    #expect(HostDiskSpace.shortfall(writableDisks: [Path(first), Path(other)], probe: probe) == nil)
    // Two disks on volume A need 200 MiB there.
    let shortfall = HostDiskSpace.shortfall(
        writableDisks: [Path(first), Path(second), Path(other)], probe: probe)
    #expect(shortfall?.requiredBytes == 200 * mib)
    #expect(shortfall?.availableBytes == 150 * mib)
    #expect(shortfall?.volume.hasSuffix("/a/") == true)
}

@Test("A volume whose free space cannot be read does not block the start")
func unknownVolumeDoesNotBlock() throws {
    let dir = try makeTempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let disk = dir.appendingPathComponent("disk.img")
    try makeSparseDisk(at: disk, logicalBytes: 50 * gib)

    #expect(
        HostDiskSpace.shortfall(writableDisks: [Path(disk)], probe: FixedSpaceProbe(volumes: [:]))
            == nil)
}

@MainActor
@Test("run refuses a writable start on a nearly full host volume before the guest starts (#2767)")
func runRefusesStartWhenHostVolumeIsNearlyFull() async throws {
    let storage = try makeTempDirectory()
    defer { try? FileManager.default.removeItem(at: storage) }

    let available = 115 * mib
    let factory = RecordingVMFactory()
    let controller = LumeController(
        home: Home(fileManager: .default),
        vmFactory: factory,
        hostVolumeSpaceProbe: FixedSpaceProbe(volumes: ["/": available]))

    let name = "low-space-\(UUID().uuidString.prefix(8).lowercased())"
    let vmDir = try controller.home.getVMDirectoryFromPath(name, storagePath: storage.path)
    try FileManager.default.createDirectory(at: vmDir.dir.url, withIntermediateDirectories: true)
    try makeSparseDisk(at: vmDir.diskPath.url, logicalBytes: 50 * gib)
    try Data(repeating: 0, count: 1024).write(to: vmDir.nvramPath.url)
    try vmDir.saveConfig(
        VMConfig(os: "linux", cpuCount: 1, memorySize: 1024, diskSize: 50 * gib, display: "1024x768"))
    let configBefore = try Data(contentsOf: vmDir.configPath.url)
    let diskSizeBefore = try FileManager.default.attributesOfItem(atPath: vmDir.diskPath.path)[.size]
        as? UInt64
    defer { SharedVM.shared.removeVM(name: name) }

    do {
        try await controller.runVM(
            name: name, noDisplay: true, storage: storage.path, vncPolicy: .disabled)
        Issue.record("Expected the start to be refused for lack of host space")
    } catch VMError.insufficientHostSpace(let vmName, _, let required, let free) {
        #expect(vmName == name)
        #expect(required == HostDiskSpace.reserveBytes)
        #expect(free == available)
        let message = VMError.insufficientHostSpace(
            name: vmName, volume: storage.path, required: required, available: free
        ).localizedDescription
        #expect(message.contains("\(required) bytes"))
        #expect(message.contains("\(free) bytes"))
    }

    // No virtual machine was configured, so the guest never ran.
    #expect(factory.services.isEmpty)
    #expect(SharedVM.shared.getVM(name: name) == nil)
    #expect(try Data(contentsOf: vmDir.configPath.url) == configBefore)
    #expect(
        try FileManager.default.attributesOfItem(atPath: vmDir.diskPath.path)[.size] as? UInt64
            == diskSizeBefore)
}

@MainActor
@Test("run starts the guest when the host volume holds the reserve")
func runStartsWhenHostVolumeHasRoom() async throws {
    let storage = try makeTempDirectory()
    defer { try? FileManager.default.removeItem(at: storage) }

    let factory = RecordingVMFactory()
    let controller = LumeController(
        home: Home(fileManager: .default),
        vmFactory: factory,
        hostVolumeSpaceProbe: FixedSpaceProbe(volumes: ["/": HostDiskSpace.reserveBytes]))

    let name = "roomy-\(UUID().uuidString.prefix(8).lowercased())"
    let vmDir = try controller.home.getVMDirectoryFromPath(name, storagePath: storage.path)
    try FileManager.default.createDirectory(at: vmDir.dir.url, withIntermediateDirectories: true)
    try makeSparseDisk(at: vmDir.diskPath.url, logicalBytes: 50 * gib)
    try Data(repeating: 0, count: 1024).write(to: vmDir.nvramPath.url)
    var config = try VMConfig(
        os: "linux", cpuCount: 1, memorySize: 1024, diskSize: 50 * gib, display: "1024x768")
    config.setMacAddress("00:11:22:33:44:58")
    try vmDir.saveConfig(config)
    defer { SharedVM.shared.removeVM(name: name) }

    let run = Task { @MainActor in
        try await controller.runVM(
            name: name, noDisplay: true, storage: storage.path, vncPolicy: .disabled)
    }
    let clock = ContinuousClock()
    let deadline = clock.now.advanced(by: .seconds(5))
    while factory.services.first?.state != .running, clock.now < deadline {
        try await Task.sleep(for: .milliseconds(10))
    }
    let service = try #require(factory.services.first)
    #expect(service.state == .running)
    service.simulateGuestStop()
    try await run.value
}
