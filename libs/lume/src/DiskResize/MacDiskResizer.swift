import Darwin
import Foundation

// MARK: - Options and marker

struct DiskResizeOptions {
  var backup: Bool = true
  var keepBackup: Bool = false
  var dryRun: Bool = false
  // verifyBoot is reserved for a future headless boot check.
}

/// Persistent, crash-durable record that a resize transaction is underway.
/// Its presence blocks `run`/`clone`/`push` and drives auto-recovery.
struct ResizeMarker: Codable {
  static let currentVersion = 1

  var version: Int
  var phase: Int
  var oldSizeBytes: UInt64
  var newSizeBytes: UInt64
  var backupDiskPath: String?
  var startedAt: Double
}

// MARK: - Container grow backend (injectable for tests)

/// Abstracts the read-only preflight inspection and the APFS container growth
/// (the parts that require attaching the image via hdiutil/diskutil), so the
/// pure file-surgery phases can be unit-tested with a mock.
@MainActor
protocol ContainerGrower {
  /// Read-only preflight: refuse if already attached or FileVault-encrypted.
  func inspect(diskPath: URL) throws
  /// Phase 5: grow the main APFS container into the free gap that now sits
  /// directly before the relocated recovery partition (`newRecoveryFirstLBA`).
  func grow(diskPath: URL, newRecoveryFirstLBA: UInt64) throws
}

// MARK: - Resizer

/// Recovery-preserving offline macOS disk expansion. See the decision memo:
/// grows the image sparsely, relocates the recovery partition byte-for-byte to
/// the new end of disk, rewrites both GPT copies crash-safely, then grows the
/// main APFS container into the now-adjacent gap — all with the VM stopped.
@MainActor
final class MacDiskResizer {
  private let vmDir: VMDirectory
  private let diskPath: URL
  private let vmName: String
  private let grower: ContainerGrower

  init(
    vmDir: VMDirectory,
    diskPath: URL,
    vmName: String,
    grower: ContainerGrower? = nil
  ) {
    self.vmDir = vmDir
    self.diskPath = diskPath
    self.vmName = vmName
    self.grower = grower ?? DiskutilContainerGrower()
  }

  // MARK: Recovery of an interrupted transaction

  /// If a marker is present, roll back from the backup (if any) and return
  /// true. Throws `resizeInProgress` when no backup exists to recover from.
  @discardableResult
  func recoverPendingIfAny() throws -> Bool {
    guard let marker = try vmDir.loadResizeMarker() else { return false }
    Logger.info(
      "Found interrupted disk resize; rolling back",
      metadata: ["name": vmName, "phase": "\(marker.phase)"])
    if let backup = marker.backupDiskPath, FileManager.default.fileExists(atPath: backup) {
      try restoreFromBackup()
      try vmDir.clearResizeMarker()
      deleteBackup()
      Logger.info("Disk restored from pre-resize backup", metadata: ["name": vmName])
      return true
    }
    throw DiskResizeError.resizeInProgress(vmName)
  }

  // MARK: Main entry

  func resize(to newSize: UInt64, options: DiskResizeOptions) throws {
    guard let transactionLock = try vmDir.tryAcquireResizeGuard(exclusive: true) else {
      throw DiskResizeError.vmRunning(vmName)
    }
    defer {
      flock(transactionLock.fileDescriptor, LOCK_UN)
      try? transactionLock.close()
    }

    // Hold an exclusive lock on config.json for the whole transaction.
    let lockHandle = try acquireLock()
    defer {
      flock(lockHandle.fileDescriptor, LOCK_UN)
      try? lockHandle.close()
    }

    // Recover only after proving no running VM owns the config lock. The
    // marker remains present throughout recovery, so new run attempts fail.
    if try recoverPendingIfAny() {
      Logger.info(
        "Recovered a prior interrupted resize before starting a new one",
        metadata: ["name": vmName])
    }

    let handle = try FileHandle(forUpdating: diskPath)
    defer { try? handle.close() }

    // ---- Phase 0: preflight (read-only) ----
    let fileSize = fileLogicalSize()
    let table = try GPTTable.read(from: handle, fileSizeBytes: fileSize)
    let layout = try MacDiskLayout.fingerprint(table, fileSizeBytes: fileSize)

    try validateSizeRules(newSize: newSize, fileSize: fileSize, layout: layout)
    let plan = try computePlan(newSize: newSize, layout: layout)
    try checkHostSpace(handle: handle, layout: layout)
    try grower.inspect(diskPath: diskPath)

    if options.dryRun {
      let recoveryAllocated = SectorCopier.allocatedBytes(
        fd: handle.fileDescriptor,
        startLBA: layout.recovery.startingLBA,
        lengthSectors: layout.recoveryLength)
      Logger.info(
        "Dry run: disk expansion is valid; no changes were made",
        metadata: [
          "name": vmName,
          "oldSizeBytes": "\(fileSize)",
          "newSizeBytes": "\(newSize)",
          "oldTotalSectors": "\(layout.totalSectors)",
          "newTotalSectors": "\(plan.newTotalSectors)",
          "newLastUsableLBA": "\(plan.newLastUsable)",
          "recoveryOldStart": "\(layout.recovery.startingLBA)",
          "recoveryOldEnd": "\(layout.recovery.endingLBA)",
          "recoveryNewStart": "\(plan.newRecoveryFirst)",
          "recoveryNewEnd": "\(plan.newRecoveryFirst + layout.recoveryLength - 1)",
          "recoveryLengthSectors": "\(layout.recoveryLength)",
          "recoveryAllocatedBytes": "\(recoveryAllocated)",
          "mainEnd": "\(layout.main.endingLBA)",
        ])
      return
    }

    Logger.info(
      "Starting recovery-preserving disk expansion",
      metadata: [
        "name": vmName,
        "oldSizeBytes": "\(fileSize)",
        "newSizeBytes": "\(newSize)",
        "recoveryOldStart": "\(layout.recovery.startingLBA)",
        "recoveryNewStart": "\(plan.newRecoveryFirst)",
      ])

    // ---- Phase 1: backup ----
    if options.backup {
      try makeBackup()
    } else {
      Logger.info(
        "Skipping pre-resize backup (--no-backup); rollback will not be automatic",
        metadata: ["name": vmName])
    }

    // ---- Phase 2: arm the transaction ----
    var marker = ResizeMarker(
      version: ResizeMarker.currentVersion,
      phase: 2,
      oldSizeBytes: fileSize,
      newSizeBytes: newSize,
      backupDiskPath: options.backup ? vmDir.diskBackupPath.path : nil,
      startedAt: nowTimestamp())
    try vmDir.saveResizeMarker(marker)

    do {
      // ---- Phase 3: grow + relocate ----
      marker.phase = 3
      try vmDir.saveResizeMarker(marker)
      if newSize > fileSize {
        try handle.truncate(atOffset: newSize)
      }
      let preMoveHash = try SectorCopier.copyRange(
        fd: handle.fileDescriptor,
        srcLBA: layout.recovery.startingLBA,
        dstLBA: plan.newRecoveryFirst,
        lengthSectors: layout.recoveryLength)
      try GPT.fullSync(handle)

      // Free the old recovery region now, BEFORE the container grows into
      // it, so punching a hole cannot clobber freshly-written APFS metadata.
      SectorCopier.punchHole(
        fd: handle.fileDescriptor,
        startLBA: layout.recovery.startingLBA,
        lengthSectors: layout.recoveryLength)

      // ---- Phase 4: GPT commit ----
      marker.phase = 4
      try vmDir.saveResizeMarker(marker)
      var newTable = table
      newTable.header.alternateLBA = plan.newTotalSectors - 1
      newTable.header.lastUsableLBA = plan.newLastUsable
      newTable.entries[MacDiskLayout.recoveryIndex].startingLBA = plan.newRecoveryFirst
      newTable.entries[MacDiskLayout.recoveryIndex].endingLBA =
        plan.newRecoveryFirst + layout.recoveryLength - 1
      try newTable.write(to: handle)
      try GPT.fullSync(handle)

      // ---- Phase 5: APFS container grow ----
      marker.phase = 5
      try vmDir.saveResizeMarker(marker)
      try grower.grow(diskPath: diskPath, newRecoveryFirstLBA: plan.newRecoveryFirst)

      // ---- Phase 6: verify + commit ----
      marker.phase = 6
      try vmDir.saveResizeMarker(marker)
      try verify(preMoveHash: preMoveHash, plan: plan, layout: layout)
      try updateConfigDiskSize(newSize)

      // Success: clean up.
      // Remove the guard first. A crash after this point can leave an
      // unused backup, but never a marker whose recovery copy is gone.
      try vmDir.clearResizeMarker()
      if options.backup && !options.keepBackup { deleteBackup() }
      Logger.info(
        "Disk expansion completed",
        metadata: ["name": vmName, "newSizeBytes": "\(newSize)"])
    } catch {
      do {
        try rollback(afterError: error, hadBackup: options.backup)
      } catch {
        throw error
      }
      throw error
    }
  }

  // MARK: - Phase 0 helpers

  private struct Plan {
    let newTotalSectors: UInt64
    let newLastUsable: UInt64
    let newRecoveryFirst: UInt64
  }

  private func validateSizeRules(newSize: UInt64, fileSize: UInt64, layout: MacDiskLayout) throws {
    guard newSize % 4096 == 0 else {
      throw DiskResizeError.unsupportedLayout(
        "requested size \(newSize) is not a multiple of 4096 bytes")
    }
    guard newSize >= fileSize else {
      throw DiskResizeError.shrinkNotSupported(current: fileSize, requested: newSize)
    }
    guard newSize > layout.gptTotalBytes else {
      throw DiskResizeError.shrinkNotSupported(
        current: layout.gptTotalBytes, requested: newSize)
    }
    // Non-overlap: the new recovery region must start past the old one.
    let minGrowthSectors = layout.recoveryLength + 34
    let minNewSizeRaw = layout.gptTotalBytes + minGrowthSectors * UInt64(GPTTable.sectorSize)
    let minNewSize = roundUp(minNewSizeRaw, to: 4096)
    guard newSize >= minNewSize else {
      throw DiskResizeError.growthTooSmall(minimumBytes: minNewSize)
    }
  }

  private func computePlan(newSize: UInt64, layout: MacDiskLayout) throws -> Plan {
    let ss = UInt64(GPTTable.sectorSize)
    let newTotalSectors = newSize / ss
    let newLastUsable = newTotalSectors - 34
    let recLen = layout.recoveryLength
    // Hug the end with 0–7 sectors of slack, 8-sector aligned start.
    let newRecoveryFirst = floorAlign(newLastUsable - recLen + 1, to: MacDiskLayout.alignment)
    guard newRecoveryFirst > layout.recovery.endingLBA else {
      throw DiskResizeError.growthTooSmall(
        minimumBytes: roundUp(
          layout.gptTotalBytes + (recLen + 34) * ss, to: 4096))
    }
    guard newRecoveryFirst + recLen - 1 <= newLastUsable else {
      throw DiskResizeError.unsupportedLayout("internal: relocated recovery exceeds LastUsable")
    }
    return Plan(
      newTotalSectors: newTotalSectors,
      newLastUsable: newLastUsable,
      newRecoveryFirst: newRecoveryFirst)
  }

  private func checkHostSpace(handle: FileHandle, layout: MacDiskLayout) throws {
    let needed =
      SectorCopier.allocatedBytes(
        fd: handle.fileDescriptor,
        startLBA: layout.recovery.startingLBA,
        lengthSectors: layout.recoveryLength) + 64 * 1024 * 1024  // margin
    let available: UInt64
    if let values = try? diskPath.resourceValues(forKeys: [
      .volumeAvailableCapacityForImportantUsageKey
    ]), let capacity = values.volumeAvailableCapacityForImportantUsage {
      available = UInt64(max(0, capacity))
    } else {
      available = .max  // cannot determine; do not block
    }
    guard available >= needed else {
      throw DiskResizeError.insufficientHostSpace(needed: needed, available: available)
    }
  }

  // MARK: - Phase 6 verify

  private func verify(preMoveHash: Data, plan: Plan, layout: MacDiskLayout) throws {
    let handle = try FileHandle(forReadingFrom: diskPath)
    defer { try? handle.close() }
    let fileSize = fileLogicalSize()
    let table = try GPTTable.read(from: handle, fileSizeBytes: fileSize)
    let newLayout = try MacDiskLayout.fingerprint(table, fileSizeBytes: fileSize)
    guard newLayout.recovery.startingLBA == plan.newRecoveryFirst else {
      throw DiskResizeError.verificationFailed(
        "recovery partition did not land at LBA \(plan.newRecoveryFirst)")
    }
    let postHash = try SectorCopier.hashRange(
      fd: handle.fileDescriptor,
      startLBA: plan.newRecoveryFirst,
      lengthSectors: layout.recoveryLength)
    guard postHash == preMoveHash else {
      throw DiskResizeError.verificationFailed(
        "relocated recovery bytes do not match the pre-move hash")
    }
  }

  // MARK: - Backup / rollback

  private func makeBackup() throws {
    deleteBackup()
    let result = Darwin.clonefile(diskPath.path, vmDir.diskBackupPath.path, 0)
    if result != 0 {
      let code = errno
      throw DiskResizeError.backupFailed(
        "clonefile failed (errno \(code): \(String(cString: strerror(code))))")
    }
    // Copy config.json alongside so it can be restored atomically.
    try? FileManager.default.removeItem(atPath: vmDir.configBackupPath.path)
    do {
      try FileManager.default.copyItem(
        at: vmDir.configPath.url, to: vmDir.configBackupPath.url)
    } catch {
      throw DiskResizeError.backupFailed(
        "could not back up config.json: \(error.localizedDescription)")
    }
  }

  private func deleteBackup() {
    try? FileManager.default.removeItem(atPath: vmDir.diskBackupPath.path)
    try? FileManager.default.removeItem(atPath: vmDir.configBackupPath.path)
  }

  private func restoreFromBackup() throws {
    guard FileManager.default.fileExists(atPath: vmDir.diskBackupPath.path),
      FileManager.default.fileExists(atPath: vmDir.configBackupPath.path)
    else {
      throw DiskResizeError.backupFailed("rollback backup is incomplete")
    }
    let diskRestore = diskPath.path + ".restore"
    let configRestore = vmDir.configPath.path + ".restore"
    try? FileManager.default.removeItem(atPath: diskRestore)
    try? FileManager.default.removeItem(atPath: configRestore)
    guard Darwin.clonefile(vmDir.diskBackupPath.path, diskRestore, 0) == 0 else {
      throw DiskResizeError.backupFailed(
        "could not stage disk restore: \(String(cString: strerror(errno)))")
    }
    do {
      try FileManager.default.copyItem(
        atPath: vmDir.configBackupPath.path, toPath: configRestore)
    } catch {
      try? FileManager.default.removeItem(atPath: diskRestore)
      throw DiskResizeError.backupFailed(
        "could not stage config restore: \(error.localizedDescription)")
    }
    guard Darwin.rename(configRestore, vmDir.configPath.path) == 0 else {
      try? FileManager.default.removeItem(atPath: diskRestore)
      throw DiskResizeError.backupFailed(
        "could not restore config.json: \(String(cString: strerror(errno)))")
    }
    guard Darwin.rename(diskRestore, diskPath.path) == 0 else {
      throw DiskResizeError.backupFailed(
        "could not restore disk.img: \(String(cString: strerror(errno)))")
    }
  }

  private func rollback(afterError error: Error, hadBackup: Bool) throws {
    let originalDescription = error.localizedDescription
    Logger.error(
      "Disk expansion failed; rolling back",
      metadata: ["name": vmName, "error": error.localizedDescription])
    if hadBackup, FileManager.default.fileExists(atPath: vmDir.diskBackupPath.path) {
      do {
        try restoreFromBackup()
        try vmDir.clearResizeMarker()
        deleteBackup()
        Logger.info("Disk restored from pre-resize backup", metadata: ["name": vmName])
      } catch let restoreError {
        throw DiskResizeError.rollbackFailed(
          original: originalDescription, restore: restoreError.localizedDescription)
      }
    } else {
      Logger.error(
        "No backup available; leaving resize marker for manual recovery",
        metadata: ["name": vmName])
    }
  }

  // MARK: - Config

  private func updateConfigDiskSize(_ newSize: UInt64) throws {
    var config = try vmDir.loadConfig()
    config.setDiskSize(newSize)
    try vmDir.saveConfig(config)
  }

  // MARK: - Locking

  private func acquireLock() throws -> FileHandle {
    let handle: FileHandle
    do {
      handle = try FileHandle(forWritingTo: vmDir.configPath.url)
    } catch {
      throw DiskResizeError.commandFailed(
        command: "open config.json", output: error.localizedDescription)
    }
    guard flock(handle.fileDescriptor, LOCK_EX | LOCK_NB) == 0 else {
      try? handle.close()
      throw DiskResizeError.vmRunning(vmName)
    }
    return handle
  }

  // MARK: - Misc

  private func fileLogicalSize() -> UInt64 {
    ((try? FileManager.default.attributesOfItem(atPath: diskPath.path))?[.size]
      as? NSNumber)?.uint64Value ?? 0
  }

  private func nowTimestamp() -> Double {
    Date().timeIntervalSince1970
  }
}

// MARK: - Helpers

private func floorAlign(_ value: UInt64, to alignment: UInt64) -> UInt64 {
  value - (value % alignment)
}

private func roundUp(_ value: UInt64, to alignment: UInt64) -> UInt64 {
  let remainder = value % alignment
  return remainder == 0 ? value : value + (alignment - remainder)
}
