import Foundation

/// Real `ContainerGrower` that attaches the raw image with hdiutil and grows the
/// main APFS container with `diskutil apfs resizeContainer`. Falls back to
/// pre-growing the partition entry (Plan B) if the one-shot grow (Plan A) will
/// not fill the adjacent gap on its own.
@MainActor
final class DiskutilContainerGrower: ContainerGrower {
  func inspect(diskPath: URL) throws {
    let session = DiskImageSession(diskPath: diskPath.path)
    if (try? session.isAttached()) == true {
      throw DiskResizeError.imageAttached
    }
    let wholeDisk = try session.attach(readWrite: false)
    defer { session.detach() }
    let container = try session.apfsContainer(forMainPartitionOf: wholeDisk)
    if try isEncrypted(session: session, container: container) {
      throw DiskResizeError.fileVaultEnabled
    }
  }

  func grow(diskPath: URL, newRecoveryFirstLBA: UInt64) throws {
    let handle = try FileHandle(forReadingFrom: diskPath)
    let fileSize =
      (try FileManager.default.attributesOfItem(atPath: diskPath.path)[.size]
      as? NSNumber)?.uint64Value ?? 0
    let table = try GPTTable.read(from: handle, fileSizeBytes: fileSize)
    try handle.close()
    let expectedSize =
      (newRecoveryFirstLBA - table.entries[MacDiskLayout.mainIndex].startingLBA)
      * UInt64(GPTTable.sectorSize)

    // Plan A: diskutil normally stretches both P2 and its APFS container.
    if try resizeContainer(diskPath: diskPath, expectedSize: expectedSize) { return }

    Logger.info("resizeContainer did not fill the gap; pre-growing partition 2")
    try preGrowMainPartition(diskPath: diskPath, upToLBA: newRecoveryFirstLBA - 1)
    guard try resizeContainer(diskPath: diskPath, expectedSize: expectedSize) else {
      throw DiskResizeError.verificationFailed(
        "APFS physical store did not reach \(expectedSize) bytes")
    }
  }

  // MARK: - diskutil

  private func resizeContainer(diskPath: URL, expectedSize: UInt64) throws -> Bool {
    let session = DiskImageSession(diskPath: diskPath.path)
    let wholeDisk = try session.attach(readWrite: true)
    defer {
      session.unmountDiskIfMounted()
      session.detach()
    }
    do {
      _ = try session.run(
        "/usr/sbin/diskutil", ["apfs", "resizeContainer", "\(wholeDisk)s2", "0"])
      let container = try session.apfsContainer(forMainPartitionOf: wholeDisk)
      let list = try session.runPlist(
        "/usr/sbin/diskutil", ["apfs", "list", "-plist", container])
      let containers = list["Containers"] as? [[String: Any]] ?? []
      let matching = containers.first {
        ($0["ContainerReference"] as? String) == container
          || ($0["APFSContainerReference"] as? String) == container
      }
      let actualSize = (matching?["CapacityCeiling"] as? NSNumber)?.uint64Value ?? 0
      return actualSize >= expectedSize
    } catch DiskImageError.commandFailed(let command, let output) {
      if looksLikeDirtyContainer(output) {
        throw DiskResizeError.apfsCheckFailed(output)
      }
      throw DiskResizeError.commandFailed(command: command, output: output)
    }
  }

  private func preGrowMainPartition(diskPath: URL, upToLBA endLBA: UInt64) throws {
    let handle = try FileHandle(forUpdating: diskPath)
    defer { try? handle.close() }
    let fileSize =
      ((try? FileManager.default.attributesOfItem(atPath: diskPath.path))?[.size]
      as? NSNumber)?.uint64Value ?? 0
    var table = try GPTTable.read(from: handle, fileSizeBytes: fileSize)
    // Validate we still have the expected layout (recovery already relocated).
    _ = try MacDiskLayout.fingerprint(table, fileSizeBytes: fileSize)
    guard endLBA > table.entries[MacDiskLayout.mainIndex].endingLBA else { return }
    table.entries[MacDiskLayout.mainIndex].endingLBA = endLBA
    try table.write(to: handle)
    try GPT.fullSync(handle)
  }

  private func isEncrypted(session: DiskImageSession, container: String) throws -> Bool {
    let plist = try session.runPlist(
      "/usr/sbin/diskutil", ["apfs", "list", "-plist", container])
    guard let containers = plist["Containers"] as? [[String: Any]] else { return false }
    for container in containers {
      guard let volumes = container["Volumes"] as? [[String: Any]] else { continue }
      for volume in volumes {
        if (volume["FileVault"] as? Bool) == true
          || (volume["Encryption"] as? Bool) == true
        {
          return true
        }
      }
    }
    return false
  }

  private func looksLikeDirtyContainer(_ output: String) -> Bool {
    let lowered = output.lowercased()
    return lowered.contains("could not be verified")
      || lowered.contains("fsck")
      || lowered.contains("not appear to be")
      || lowered.contains("run first aid")
      || lowered.contains("volume is not clean")
  }
}
