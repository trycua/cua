import Foundation
import Testing

@testable import lume

// MARK: - Mock growers

@MainActor
final class NoopGrower: ContainerGrower {
  var inspected = false
  var grewToRecoveryFirst: UInt64?
  func inspect(diskPath: URL) throws { inspected = true }
  func grow(diskPath: URL, newRecoveryFirstLBA: UInt64) throws {
    grewToRecoveryFirst = newRecoveryFirstLBA
  }
}

@MainActor
final class ThrowingGrower: ContainerGrower {
  func inspect(diskPath: URL) throws {}
  func grow(diskPath: URL, newRecoveryFirstLBA: UInt64) throws {
    throw DiskResizeError.containerGrowFailed("simulated Phase 5 failure")
  }
}

// MARK: - Fixtures for a full VM dir

@MainActor
enum ResizerFixtures {
  /// Builds a temp VM directory containing a fixture `disk.img` and a valid
  /// macOS `config.json`, returning the directory, the resizer, and the disk URL.
  static func makeVMDir(
    totalSectors: UInt64 = GPTFixtures.defaultTotalSectors,
    oldDiskSize: UInt64,
    grower: ContainerGrower
  ) throws -> (vmDir: VMDirectory, resizer: MacDiskResizer, diskURL: URL) {
    let root = FileManager.default.temporaryDirectory
      .appendingPathComponent("lume-vm-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)

    let (fixtureURL, _) = try GPTFixtures.makeDisk(totalSectors: totalSectors)
    let diskURL = root.appendingPathComponent("disk.img")
    try FileManager.default.moveItem(at: fixtureURL, to: diskURL)

    let vmDir = VMDirectory(Path(root.path))
    let config = try VMConfig(os: "macOS", diskSize: oldDiskSize, display: "1024x768")
    try vmDir.saveConfig(config)

    let resizer = MacDiskResizer(
      vmDir: vmDir, diskPath: diskURL, vmName: vmDir.name, grower: grower)
    return (vmDir, resizer, diskURL)
  }

    static func remove(_ vmDir: VMDirectory) {
        try? FileManager.default.removeItem(atPath: vmDir.dir.path)
        try? FileManager.default.removeItem(atPath: vmDir.resizeGuardPath.path)
    }
}

// MARK: - Happy path

@MainActor
@Suite("MacDiskResizer happy path")
struct MacDiskResizerHappyPathTests {
  // 64 MiB → 96 MiB.
  let oldSize: UInt64 = 64 * 1024 * 1024
  let newSize: UInt64 = 96 * 1024 * 1024

  @Test("expands, relocates recovery byte-identically, updates config and cleans up")
  func happyPath() throws {
    let grower = NoopGrower()
    let (vmDir, resizer, diskURL) = try ResizerFixtures.makeVMDir(
      oldDiskSize: oldSize, grower: grower)
    defer { ResizerFixtures.remove(vmDir) }

    // Capture the pre-move recovery hash from the original layout.
    let preHandle = try FileHandle(forReadingFrom: diskURL)
    let preTable = try GPTTable.read(from: preHandle, fileSizeBytes: oldSize)
    let preLayout = try MacDiskLayout.fingerprint(preTable, fileSizeBytes: oldSize)
    let preHash = try SectorCopier.hashRange(
      fd: preHandle.fileDescriptor,
      startLBA: preLayout.recovery.startingLBA,
      lengthSectors: preLayout.recoveryLength)
    try preHandle.close()

    try resizer.resize(to: newSize, options: DiskResizeOptions())

    // File grew.
    #expect(GPTFixtures.fileSize(diskURL) == newSize)

    // GPT re-fingerprints with recovery at the new tail; hash preserved.
    let handle = try FileHandle(forReadingFrom: diskURL)
    defer { try? handle.close() }
    let table = try GPTTable.read(from: handle, fileSizeBytes: newSize)
    let layout = try MacDiskLayout.fingerprint(table, fileSizeBytes: newSize)
    let expectedTotal = newSize / 512
    #expect(layout.totalSectors == expectedTotal)
    #expect(table.header.lastUsableLBA == expectedTotal - 34)
    #expect(layout.recovery.startingLBA > preLayout.recovery.endingLBA)
    #expect(layout.recovery.startingLBA % 8 == 0)
    #expect(layout.recoveryLength == preLayout.recoveryLength)
    // Same recovery identity (unique GUID + name preserved).
    #expect(layout.recovery.uniqueGUID == preLayout.recovery.uniqueGUID)
    #expect(layout.recovery.nameRaw == preLayout.recovery.nameRaw)
    // Recovery hugs LastUsable within 7 sectors.
    #expect(table.header.lastUsableLBA - layout.recovery.endingLBA <= 7)

    let postHash = try SectorCopier.hashRange(
      fd: handle.fileDescriptor,
      startLBA: layout.recovery.startingLBA,
      lengthSectors: layout.recoveryLength)
    #expect(postHash == preHash)

    // Grower was driven with the relocated recovery start.
    #expect(grower.grewToRecoveryFirst == layout.recovery.startingLBA)

    // Config updated; marker + backup cleaned up.
    let config = try vmDir.loadConfig()
    #expect(config.diskSize == newSize)
    #expect(!vmDir.hasResizeMarker())
    #expect(!FileManager.default.fileExists(atPath: vmDir.diskBackupPath.path))
  }

  @Test("--keep-backup leaves the pre-resize backup in place")
  func keepBackup() throws {
    let (vmDir, resizer, _) = try ResizerFixtures.makeVMDir(
      oldDiskSize: oldSize, grower: NoopGrower())
    defer { ResizerFixtures.remove(vmDir) }
    try resizer.resize(to: newSize, options: DiskResizeOptions(backup: true, keepBackup: true))
    #expect(FileManager.default.fileExists(atPath: vmDir.diskBackupPath.path))
    #expect(!vmDir.hasResizeMarker())
  }

  @Test("dry-run leaves disk and configuration unchanged")
  func dryRun() throws {
    let (vmDir, resizer, diskURL) = try ResizerFixtures.makeVMDir(
      oldDiskSize: oldSize, grower: NoopGrower())
    defer { ResizerFixtures.remove(vmDir) }
    let original = try Data(contentsOf: diskURL)
    try resizer.resize(to: newSize, options: DiskResizeOptions(dryRun: true))
    #expect(try Data(contentsOf: diskURL) == original)
    #expect(try vmDir.loadConfig().diskSize == oldSize)
    #expect(!vmDir.hasResizeMarker())
  }
}

// MARK: - Guards & rollback

@MainActor
@Suite("MacDiskResizer guards and rollback")
struct MacDiskResizerGuardTests {
  let oldSize: UInt64 = 64 * 1024 * 1024

  @Test("shrink is refused")
  func shrink() throws {
    let (vmDir, resizer, _) = try ResizerFixtures.makeVMDir(
      oldDiskSize: oldSize, grower: NoopGrower())
    defer { ResizerFixtures.remove(vmDir) }
    #expect(throws: DiskResizeError.self) {
      try resizer.resize(to: 32 * 1024 * 1024, options: DiskResizeOptions())
    }
    #expect(!vmDir.hasResizeMarker())
  }

  @Test("growth smaller than the recovery relocation minimum is refused")
  func growthTooSmall() throws {
    let (vmDir, resizer, _) = try ResizerFixtures.makeVMDir(
      oldDiskSize: oldSize, grower: NoopGrower())
    defer { ResizerFixtures.remove(vmDir) }
    // +1 MiB is far below recovery(≈4 MiB) + slop.
    #expect(throws: DiskResizeError.self) {
      try resizer.resize(to: oldSize + 1024 * 1024, options: DiskResizeOptions())
    }
    #expect(!vmDir.hasResizeMarker())
  }

  @Test("a held config.json lock is treated as a running VM")
  func runningLock() throws {
    let (vmDir, resizer, _) = try ResizerFixtures.makeVMDir(
      oldDiskSize: oldSize, grower: NoopGrower())
    defer { ResizerFixtures.remove(vmDir) }
    let lock = try FileHandle(forWritingTo: vmDir.configPath.url)
    defer {
      flock(lock.fileDescriptor, LOCK_UN)
      try? lock.close()
    }

    #expect(flock(lock.fileDescriptor, LOCK_EX | LOCK_NB) == 0)
    #expect(throws: DiskResizeError.self) {
      try resizer.resize(to: 96 * 1024 * 1024, options: DiskResizeOptions())
    }
  }

  @Test("a shared VM-operation guard blocks resize")
  func sharedGuardBlocksResize() throws {
    let (vmDir, resizer, _) = try ResizerFixtures.makeVMDir(
      oldDiskSize: oldSize, grower: NoopGrower())
    defer { ResizerFixtures.remove(vmDir) }
    let acquired = try vmDir.tryAcquireResizeGuard(exclusive: false)
    let guardHandle = try #require(acquired)
    defer {
      flock(guardHandle.fileDescriptor, LOCK_UN)
      try? guardHandle.close()
    }
    #expect(throws: DiskResizeError.self) {
      try resizer.resize(to: 96 * 1024 * 1024, options: DiskResizeOptions())
    }
  }

  @Test("a Phase 5 failure rolls back to the byte-identical original disk")
  func rollbackOnGrowFailure() throws {
    let (vmDir, resizer, diskURL) = try ResizerFixtures.makeVMDir(
      oldDiskSize: oldSize, grower: ThrowingGrower())
    defer { ResizerFixtures.remove(vmDir) }
    let original = try Data(contentsOf: diskURL)

    #expect(throws: DiskResizeError.self) {
      try resizer.resize(to: 96 * 1024 * 1024, options: DiskResizeOptions())
    }
    // Disk restored byte-for-byte; marker + backup gone; config unchanged.
    let restored = try Data(contentsOf: diskURL)
    #expect(restored == original)
    #expect(!vmDir.hasResizeMarker())
    #expect(!FileManager.default.fileExists(atPath: vmDir.diskBackupPath.path))
    #expect(try vmDir.loadConfig().diskSize == oldSize)
  }

  @Test("an interrupted transaction with a backup auto-recovers on next invocation")
  func recoverPending() throws {
    let (vmDir, resizer, diskURL) = try ResizerFixtures.makeVMDir(
      oldDiskSize: oldSize, grower: NoopGrower())
    defer { ResizerFixtures.remove(vmDir) }
    let original = try Data(contentsOf: diskURL)

    // Simulate a crash mid-transaction: a backup exists and a marker is armed,
    // while the live disk has been mangled.
    _ = Darwin.clonefile(diskURL.path, vmDir.diskBackupPath.path, 0)
    try FileManager.default.copyItem(
      at: vmDir.configPath.url, to: vmDir.configBackupPath.url)
    let mangle = try FileHandle(forWritingTo: diskURL)
    try mangle.seek(toOffset: 512)
    try mangle.write(contentsOf: Data(repeating: 0xAB, count: 512))
    try mangle.close()
    try vmDir.saveResizeMarker(
      ResizeMarker(
        version: 1, phase: 4, oldSizeBytes: oldSize, newSizeBytes: 96 * 1024 * 1024,
        backupDiskPath: vmDir.diskBackupPath.path, startedAt: 0))

    #expect(try resizer.recoverPendingIfAny() == true)
    #expect(try Data(contentsOf: diskURL) == original)
    #expect(!vmDir.hasResizeMarker())
  }

  @Test("a malformed resize marker fails closed")
  func malformedMarker() throws {
    let (vmDir, resizer, _) = try ResizerFixtures.makeVMDir(
      oldDiskSize: oldSize, grower: NoopGrower())
    defer { ResizerFixtures.remove(vmDir) }
    try Data("not-json".utf8).write(to: vmDir.resizeMarkerPath.url)
    #expect(throws: DiskResizeError.self) {
      try resizer.resize(to: 96 * 1024 * 1024, options: DiskResizeOptions())
    }
    #expect(vmDir.hasResizeMarker())
  }
}

// MARK: - VMDirectory.setDisk regression

@Suite("VMDirectory.setDisk")
struct VMDirectorySetDiskTests {
  private func tempVMDir() throws -> VMDirectory {
    let root = FileManager.default.temporaryDirectory
      .appendingPathComponent("lume-setdisk-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    return VMDirectory(Path(root.path))
  }

  @Test("grow succeeds; shrink throws instead of silently corrupting")
  func growThenShrink() throws {
    let vmDir = try tempVMDir()
    defer { try? FileManager.default.removeItem(atPath: vmDir.dir.path) }
    try vmDir.setDisk(10 * 1024 * 1024)
    #expect(GPTFixtures.fileSize(vmDir.diskPath.url) == 10 * 1024 * 1024)
    try vmDir.setDisk(20 * 1024 * 1024)  // grow OK
    #expect(GPTFixtures.fileSize(vmDir.diskPath.url) == 20 * 1024 * 1024)
    #expect(throws: VMDirectoryError.self) {
      try vmDir.setDisk(5 * 1024 * 1024)  // shrink refused
    }
    #expect(GPTFixtures.fileSize(vmDir.diskPath.url) == 20 * 1024 * 1024)
  }
}
