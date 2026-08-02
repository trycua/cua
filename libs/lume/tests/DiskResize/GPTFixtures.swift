import Foundation

@testable import lume

/// Builds synthetic but structurally-real GPT disk images in temp files for the
/// disk-resize unit tests: correct CRCs, mixed-endian Apple type GUIDs, and a
/// deterministic recovery-region byte pattern so relocation can be hash-checked.
enum GPTFixtures {
  static let sectorSize: UInt64 = 512

  /// A 64 MiB disk (131,072 sectors) laid out ISC / main APFS / recovery-last,
  /// mirroring the real Tahoe layout at small scale.
  static let defaultTotalSectors: UInt64 = 131_072

  struct Layout {
    var iscStart: UInt64 = 40
    var iscEnd: UInt64 = 2_087
    var mainStart: UInt64 = 2_088
    var mainEnd: UInt64 = 123_031
    var recoveryStart: UInt64 = 123_032
    var recoveryEnd: UInt64 = 131_031  // lastUsable(131038) - 7
  }

  static func name(_ string: String) -> [UInt8] {
    var bytes = [UInt8]()
    for unit in string.utf16 {
      bytes.append(UInt8(unit & 0xFF))
      bytes.append(UInt8((unit >> 8) & 0xFF))
    }
    while bytes.count < 72 { bytes.append(0) }
    return Array(bytes.prefix(72))
  }

  static func guid(_ string: String) -> GPTGUID { GPTGUID(uuidString: string)! }

  static func emptyEntry() -> GPTPartitionEntry {
    GPTPartitionEntry(
      typeGUID: GPTGUID(bytes: [UInt8](repeating: 0, count: 16)),
      uniqueGUID: GPTGUID(bytes: [UInt8](repeating: 0, count: 16)),
      startingLBA: 0, endingLBA: 0, attributes: 0,
      nameRaw: [UInt8](repeating: 0, count: 72))
  }

  static func standardEntries(_ layout: Layout = Layout()) -> [GPTPartitionEntry] {
    var entries = [GPTPartitionEntry](repeating: emptyEntry(), count: 128)
    entries[0] = GPTPartitionEntry(
      typeGUID: .appleAPFSISC,
      uniqueGUID: guid("9F198AF4-8CB8-48CF-9DCF-1CAE9DA11774"),
      startingLBA: layout.iscStart, endingLBA: layout.iscEnd, attributes: 0,
      nameRaw: name("iBootSystemContainer"))
    entries[1] = GPTPartitionEntry(
      typeGUID: .appleAPFS,
      uniqueGUID: guid("3833D35D-646F-4B99-BCCB-A2C3B1E4F881"),
      startingLBA: layout.mainStart, endingLBA: layout.mainEnd, attributes: 0,
      nameRaw: name("Container"))
    entries[2] = GPTPartitionEntry(
      typeGUID: .appleAPFSRecovery,
      uniqueGUID: guid("BA362677-6481-4A3F-A787-088BD36184B7"),
      startingLBA: layout.recoveryStart, endingLBA: layout.recoveryEnd, attributes: 0,
      nameRaw: name("RecoveryOSContainer"))
    return entries
  }

  static func makeHeader(totalSectors: UInt64) -> GPTHeader {
    GPTHeader(
      revision: 0x0001_0000,
      headerSize: 92,
      headerCRC32: 0,
      myLBA: 1,
      alternateLBA: totalSectors - 1,
      firstUsableLBA: 34,
      lastUsableLBA: totalSectors - 34,
      diskGUID: guid("7B049169-B6A9-4893-9D64-441B89950BFC"),
      partitionEntryLBA: 2,
      numberOfPartitionEntries: 128,
      sizeOfPartitionEntry: 128,
      partitionEntryArrayCRC32: 0)
  }

  /// Deterministic non-zero byte at a given absolute file offset.
  static func patternByte(at offset: UInt64) -> UInt8 {
    UInt8((offset &* 2_654_435_761 &+ 41) & 0xFF)
  }

  /// Creates a temp disk image, writes a valid GPT via `GPTTable.write`, and
  /// fills the recovery region with the deterministic pattern. Returns the
  /// file URL and the table that was written.
  @discardableResult
  static func makeDisk(
    totalSectors: UInt64 = defaultTotalSectors,
    layout: Layout = Layout(),
    fillRecovery: Bool = true
  ) throws -> (url: URL, table: GPTTable) {
    let url = FileManager.default.temporaryDirectory
      .appendingPathComponent("lume-gpt-\(UUID().uuidString).img")
    FileManager.default.createFile(atPath: url.path, contents: nil)
    let handle = try FileHandle(forUpdating: url)  // write() reads the MBR back
    try handle.truncate(atOffset: totalSectors * sectorSize)

    var header = makeHeader(totalSectors: totalSectors)
    header.lastUsableLBA = totalSectors - 34
    let table = GPTTable(header: header, entries: standardEntries(layout))
    try table.write(to: handle)

    if fillRecovery {
      let start = layout.recoveryStart * sectorSize
      let end = (layout.recoveryEnd + 1) * sectorSize
      let length = Int(end - start)
      var bytes = [UInt8](repeating: 0, count: length)
      for i in 0..<length { bytes[i] = patternByte(at: start + UInt64(i)) }
      try handle.seek(toOffset: start)
      try handle.write(contentsOf: Data(bytes))
    }
    try handle.close()
    return (url, table)
  }

  static func fileSize(_ url: URL) -> UInt64 {
    let attrs = try? FileManager.default.attributesOfItem(atPath: url.path)
    return (attrs?[.size] as? NSNumber)?.uint64Value ?? 0
  }

  static func remove(_ url: URL) { try? FileManager.default.removeItem(at: url) }
}
