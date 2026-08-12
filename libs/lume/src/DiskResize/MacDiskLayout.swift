import Foundation

/// The validated, known-good macOS 3-partition layout: iBootSystemContainer
/// (ISC), the main APFS container, and the RecoveryOS container last on disk.
///
/// `fingerprint` fails closed unless every expected invariant holds, so the
/// resize engine only ever operates on a layout it fully understands.
struct MacDiskLayout {
  /// 4 KiB / 512 B = 8-sector alignment used by the factory layout.
  static let alignment: UInt64 = 8
  static let iscIndex = 0
  static let mainIndex = 1
  static let recoveryIndex = 2

  let table: GPTTable
  let fileSizeBytes: UInt64

  var isc: GPTPartitionEntry { table.entries[Self.iscIndex] }
  var main: GPTPartitionEntry { table.entries[Self.mainIndex] }
  var recovery: GPTPartitionEntry { table.entries[Self.recoveryIndex] }

  var totalSectors: UInt64 { table.totalSectors }
  var firstUsableLBA: UInt64 { table.header.firstUsableLBA }
  var lastUsableLBA: UInt64 { table.header.lastUsableLBA }
  var recoveryLength: UInt64 { recovery.sectorCount }
  var gptTotalBytes: UInt64 { totalSectors * UInt64(GPTTable.sectorSize) }

  static func fingerprint(_ table: GPTTable, fileSizeBytes: UInt64) throws -> MacDiskLayout {
    func reject(_ reason: String) -> DiskResizeError { .unsupportedLayout(reason) }

    let header = table.header
    guard header.numberOfPartitionEntries == 128,
      header.sizeOfPartitionEntry == UInt32(GPTPartitionEntry.byteSize),
      header.partitionEntryLBA == 2
    else {
      throw reject("expected 128×128-byte entries at LBA 2")
    }
    guard header.firstUsableLBA == 34 else {
      throw reject("FirstUsableLBA is \(header.firstUsableLBA), expected 34")
    }

    // Exactly entries 0–2 populated.
    for i in 3..<table.entries.count where !table.entries[i].isEmpty {
      throw reject("partition entry \(i) is unexpectedly populated")
    }
    guard !table.entries[0].isEmpty, !table.entries[1].isEmpty, !table.entries[2].isEmpty else {
      throw reject("fewer than 3 populated partitions")
    }

    let isc = table.entries[iscIndex]
    let main = table.entries[mainIndex]
    let recovery = table.entries[recoveryIndex]

    guard isc.typeGUID == GPTGUID.appleAPFSISC else {
      throw reject("partition 1 is not Apple_APFS_ISC")
    }
    guard main.typeGUID == GPTGUID.appleAPFS else {
      throw reject("partition 2 is not Apple_APFS")
    }
    guard recovery.typeGUID == GPTGUID.appleAPFSRecovery else {
      throw reject("partition 3 is not Apple_APFS_Recovery")
    }

    // Ascending, non-overlapping order.
    guard isc.startingLBA < isc.endingLBA,
      main.startingLBA < main.endingLBA,
      recovery.startingLBA < recovery.endingLBA
    else {
      throw reject("a partition has a non-positive length")
    }
    guard isc.endingLBA < main.startingLBA, main.endingLBA < recovery.startingLBA else {
      throw reject("partitions are not in ascending, non-overlapping order")
    }

    // Recovery is last and hugs LastUsableLBA (0–7 sectors of slack).
    let lastUsable = header.lastUsableLBA
    guard recovery.endingLBA <= lastUsable else {
      throw reject("recovery partition extends past LastUsableLBA")
    }
    let slack = lastUsable - recovery.endingLBA
    guard slack <= 7 else {
      throw reject("recovery partition ends \(slack) sectors before LastUsableLBA (expected ≤ 7)")
    }

    // 4 KiB alignment of recovery start and length.
    guard recovery.startingLBA % alignment == 0 else {
      throw reject("recovery start LBA \(recovery.startingLBA) is not 8-sector aligned")
    }
    guard recovery.sectorCount % alignment == 0 else {
      throw reject("recovery length \(recovery.sectorCount) is not 8-sector aligned")
    }

    // GPT-described size must fit inside the file (equality = fresh disk;
    // file larger = an old `lume set` truncate we can repair on top of).
    let gptTotalBytes = table.totalSectors * UInt64(GPTTable.sectorSize)
    guard gptTotalBytes <= fileSizeBytes else {
      throw reject(
        "GPT describes \(gptTotalBytes) bytes but the file is only \(fileSizeBytes) bytes")
    }

    return MacDiskLayout(table: table, fileSizeBytes: fileSizeBytes)
  }
}
