import Darwin
import Foundation

// MARK: - CRC32

/// Standard IEEE 802.3 / zlib CRC-32 (reflected, poly 0xEDB88320). Matches the
/// checksums Apple's disk tooling writes into GPT headers.
enum CRC32 {
  private static let table: [UInt32] = {
    (0..<256).map { index -> UInt32 in
      var c = UInt32(index)
      for _ in 0..<8 {
        c = (c & 1) != 0 ? (0xEDB8_8320 ^ (c >> 1)) : (c >> 1)
      }
      return c
    }
  }()

  static func checksum(_ bytes: [UInt8]) -> UInt32 {
    var crc: UInt32 = 0xFFFF_FFFF
    for byte in bytes {
      crc = table[Int((crc ^ UInt32(byte)) & 0xFF)] ^ (crc >> 8)
    }
    return crc ^ 0xFFFF_FFFF
  }
}

// MARK: - Little-endian helpers

private func le32(_ b: [UInt8], _ o: Int) -> UInt32 {
  UInt32(b[o]) | (UInt32(b[o + 1]) << 8) | (UInt32(b[o + 2]) << 16) | (UInt32(b[o + 3]) << 24)
}

private func le64(_ b: [UInt8], _ o: Int) -> UInt64 {
  var value: UInt64 = 0
  for i in 0..<8 { value |= UInt64(b[o + i]) << (8 * i) }
  return value
}

private func putLE32(_ v: UInt32, into b: inout [UInt8], at o: Int) {
  for i in 0..<4 { b[o + i] = UInt8((v >> (8 * i)) & 0xFF) }
}

private func putLE64(_ v: UInt64, into b: inout [UInt8], at o: Int) {
  for i in 0..<8 { b[o + i] = UInt8((v >> (8 * UInt64(i))) & 0xFF) }
}

// MARK: - GPTGUID

/// A GPT GUID stored as its raw 16 on-disk bytes (mixed-endian). We never need
/// to interpret partition GUIDs beyond comparing them to known type constants
/// and preserving them byte-for-byte, so raw storage avoids any re-encoding bug.
struct GPTGUID: Equatable, CustomStringConvertible {
  let bytes: [UInt8]

  init(bytes: [UInt8]) {
    precondition(bytes.count == 16, "GPT GUID must be 16 bytes")
    self.bytes = bytes
  }

  /// Parses the canonical string form (e.g. `7C3457EF-0000-11AA-AA11-00306543ECAC`)
  /// into the mixed-endian on-disk byte layout.
  init?(uuidString: String) {
    let hex = uuidString.replacingOccurrences(of: "-", with: "")
    guard hex.count == 32 else { return nil }
    var raw = [UInt8]()
    raw.reserveCapacity(16)
    var index = hex.startIndex
    while index < hex.endIndex {
      let next = hex.index(index, offsetBy: 2)
      guard let byte = UInt8(hex[index..<next], radix: 16) else { return nil }
      raw.append(byte)
      index = next
    }
    // Data1 (4B) + Data2 (2B) + Data3 (2B) are little-endian; Data4/Data5 big-endian.
    var b = [UInt8](repeating: 0, count: 16)
    b[0] = raw[3]
    b[1] = raw[2]
    b[2] = raw[1]
    b[3] = raw[0]
    b[4] = raw[5]
    b[5] = raw[4]
    b[6] = raw[7]
    b[7] = raw[6]
    b[8] = raw[8]
    b[9] = raw[9]
    for i in 10..<16 { b[i] = raw[i] }
    self.bytes = b
  }

  var isZero: Bool { bytes.allSatisfy { $0 == 0 } }

  var description: String {
    func hex(_ range: Range<Int>) -> String {
      range.map { String(format: "%02X", bytes[$0]) }.joined()
    }
    let d1 = [bytes[3], bytes[2], bytes[1], bytes[0]].map { String(format: "%02X", $0) }.joined()
    let d2 = [bytes[5], bytes[4]].map { String(format: "%02X", $0) }.joined()
    let d3 = [bytes[7], bytes[6]].map { String(format: "%02X", $0) }.joined()
    let d4 = hex(8..<10)
    let d5 = hex(10..<16)
    return "\(d1)-\(d2)-\(d3)-\(d4)-\(d5)"
  }

  // Apple partition type GUIDs (force-unwrapped: the literals are valid).
  static let appleAPFSISC = GPTGUID(uuidString: "69646961-6700-11AA-AA11-00306543ECAC")!
  static let appleAPFS = GPTGUID(uuidString: "7C3457EF-0000-11AA-AA11-00306543ECAC")!
  static let appleAPFSRecovery = GPTGUID(uuidString: "52637672-7900-11AA-AA11-00306543ECAC")!
}

// MARK: - Partition entry

struct GPTPartitionEntry: Equatable {
  var typeGUID: GPTGUID
  var uniqueGUID: GPTGUID
  var startingLBA: UInt64
  var endingLBA: UInt64  // inclusive
  var attributes: UInt64
  var nameRaw: [UInt8]  // raw 72-byte UTF-16LE blob, kept verbatim

  var isEmpty: Bool { typeGUID.isZero }
  var sectorCount: UInt64 { endingLBA >= startingLBA ? endingLBA - startingLBA + 1 : 0 }

  static let byteSize = 128

  static func parse(_ b: [UInt8]) -> GPTPartitionEntry {
    GPTPartitionEntry(
      typeGUID: GPTGUID(bytes: Array(b[0..<16])),
      uniqueGUID: GPTGUID(bytes: Array(b[16..<32])),
      startingLBA: le64(b, 32),
      endingLBA: le64(b, 40),
      attributes: le64(b, 48),
      nameRaw: Array(b[56..<128]))
  }

  func serialized() -> [UInt8] {
    var b = [UInt8](repeating: 0, count: GPTPartitionEntry.byteSize)
    for i in 0..<16 { b[i] = typeGUID.bytes[i] }
    for i in 0..<16 { b[16 + i] = uniqueGUID.bytes[i] }
    putLE64(startingLBA, into: &b, at: 32)
    putLE64(endingLBA, into: &b, at: 40)
    putLE64(attributes, into: &b, at: 48)
    for i in 0..<72 { b[56 + i] = nameRaw[i] }
    return b
  }
}

// MARK: - Header

struct GPTHeader {
  static let signature: [UInt8] = Array("EFI PART".utf8)

  var revision: UInt32
  var headerSize: UInt32
  var headerCRC32: UInt32
  var myLBA: UInt64
  var alternateLBA: UInt64
  var firstUsableLBA: UInt64
  var lastUsableLBA: UInt64
  var diskGUID: GPTGUID
  var partitionEntryLBA: UInt64
  var numberOfPartitionEntries: UInt32
  var sizeOfPartitionEntry: UInt32
  var partitionEntryArrayCRC32: UInt32

  /// Parses a header from a 512-byte sector. Returns nil if the signature is
  /// wrong. Does not validate CRC (callers do that against `headerCRC32`).
  static func parse(sector b: [UInt8]) -> GPTHeader? {
    guard Array(b[0..<8]) == signature else { return nil }
    return GPTHeader(
      revision: le32(b, 8),
      headerSize: le32(b, 12),
      headerCRC32: le32(b, 16),
      myLBA: le64(b, 24),
      alternateLBA: le64(b, 32),
      firstUsableLBA: le64(b, 40),
      lastUsableLBA: le64(b, 48),
      diskGUID: GPTGUID(bytes: Array(b[56..<72])),
      partitionEntryLBA: le64(b, 72),
      numberOfPartitionEntries: le32(b, 80),
      sizeOfPartitionEntry: le32(b, 84),
      partitionEntryArrayCRC32: le32(b, 88))
  }

  /// True if the stored `headerCRC32` matches the recomputed value.
  func crcIsValid(sector b: [UInt8]) -> Bool {
    headerCRC32 == GPTHeader.computeHeaderCRC(sector: b, headerSize: headerSize)
  }

  static func computeHeaderCRC(sector b: [UInt8], headerSize: UInt32) -> UInt32 {
    let size = Int(headerSize)
    guard size >= 92, size <= b.count else { return 0 }
    var slice = Array(b[0..<size])
    // Zero the CRC field (offset 16..<20) before checksumming.
    for i in 16..<20 { slice[i] = 0 }
    return CRC32.checksum(slice)
  }

  /// Serializes this header into a fresh 512-byte sector, recomputing the
  /// header CRC over the first `headerSize` bytes.
  func serializedSector() -> [UInt8] {
    var b = [UInt8](repeating: 0, count: GPT.sectorSize)
    for i in 0..<8 { b[i] = GPTHeader.signature[i] }
    putLE32(revision, into: &b, at: 8)
    putLE32(headerSize, into: &b, at: 12)
    // CRC field left zero for now.
    putLE64(myLBA, into: &b, at: 24)
    putLE64(alternateLBA, into: &b, at: 32)
    putLE64(firstUsableLBA, into: &b, at: 40)
    putLE64(lastUsableLBA, into: &b, at: 48)
    for i in 0..<16 { b[56 + i] = diskGUID.bytes[i] }
    putLE64(partitionEntryLBA, into: &b, at: 72)
    putLE32(numberOfPartitionEntries, into: &b, at: 80)
    putLE32(sizeOfPartitionEntry, into: &b, at: 84)
    putLE32(partitionEntryArrayCRC32, into: &b, at: 88)
    let crc = GPTHeader.computeHeaderCRC(sector: b, headerSize: headerSize)
    putLE32(crc, into: &b, at: 16)
    return b
  }
}

// MARK: - Table

/// In-memory representation of a GPT read from a raw disk image, plus a
/// crash-safe writer. All offsets assume 512-byte logical sectors.
struct GPTTable {
  static let sectorSize = 512

  /// Primary header as read (fields are mutated by the resizer before write).
  var header: GPTHeader
  /// All `numberOfPartitionEntries` entries (populated + empty), preserved.
  var entries: [GPTPartitionEntry]

  var totalSectors: UInt64 { header.alternateLBA + 1 }

  // MARK: Read

  /// Reads and validates the primary GPT (and cross-checks the backup header)
  /// from a raw disk image file handle. Throws `DiskResizeError` on any
  /// malformation so callers get fail-closed behavior.
  static func read(from handle: FileHandle, fileSizeBytes: UInt64) throws -> GPTTable {
    let ss = UInt64(sectorSize)

    // Primary header at LBA 1.
    let headerSector = try readSector(handle, lba: 1)
    guard let header = GPTHeader.parse(sector: headerSector) else {
      // Distinguish a 4096-byte-sector image (signature at offset 4096).
      if let alt = try? readBytes(handle, offset: 4096, count: 8),
        alt == GPTHeader.signature
      {
        throw DiskResizeError.unsupportedSectorSize
      }
      throw DiskResizeError.unsupportedLayout("no GPT signature at LBA 1")
    }
    guard header.crcIsValid(sector: headerSector) else {
      throw DiskResizeError.unsupportedLayout("primary header CRC mismatch")
    }
    guard header.sizeOfPartitionEntry == UInt32(GPTPartitionEntry.byteSize),
      header.numberOfPartitionEntries >= 3, header.numberOfPartitionEntries <= 128
    else {
      throw DiskResizeError.unsupportedLayout("unexpected partition entry geometry")
    }

    // Primary entry array.
    let entriesBytes = try readBytes(
      handle,
      offset: header.partitionEntryLBA * ss,
      count: Int(header.numberOfPartitionEntries) * Int(header.sizeOfPartitionEntry))
    guard CRC32.checksum(entriesBytes) == header.partitionEntryArrayCRC32 else {
      throw DiskResizeError.unsupportedLayout("primary partition array CRC mismatch")
    }
    var entries = [GPTPartitionEntry]()
    for i in 0..<Int(header.numberOfPartitionEntries) {
      let start = i * GPTPartitionEntry.byteSize
      entries.append(
        GPTPartitionEntry.parse(Array(entriesBytes[start..<start + GPTPartitionEntry.byteSize])))
    }

    // Backup header consistency (must be inside the file).
    let backupOffset = header.alternateLBA * ss
    guard backupOffset + ss <= fileSizeBytes else {
      throw DiskResizeError.unsupportedLayout(
        "backup GPT header at LBA \(header.alternateLBA) is beyond the end of the file")
    }
    let backupSector = try readSector(handle, lba: header.alternateLBA)
    guard let backup = GPTHeader.parse(sector: backupSector),
      backup.crcIsValid(sector: backupSector)
    else {
      throw DiskResizeError.unsupportedLayout("backup GPT header missing or CRC mismatch")
    }
    guard backup.myLBA == header.alternateLBA, backup.alternateLBA == header.myLBA,
      backup.firstUsableLBA == header.firstUsableLBA,
      backup.lastUsableLBA == header.lastUsableLBA,
      backup.diskGUID == header.diskGUID,
      backup.numberOfPartitionEntries == header.numberOfPartitionEntries,
      backup.sizeOfPartitionEntry == header.sizeOfPartitionEntry,
      backup.partitionEntryArrayCRC32 == header.partitionEntryArrayCRC32
    else {
      throw DiskResizeError.unsupportedLayout("primary and backup GPT headers are inconsistent")
    }

    let backupEntriesOffset = backup.partitionEntryLBA * ss
    let entriesCount = Int(backup.numberOfPartitionEntries) * Int(backup.sizeOfPartitionEntry)
    guard backup.partitionEntryLBA > header.lastUsableLBA,
      backupEntriesOffset + UInt64(entriesCount) <= backupOffset
    else {
      throw DiskResizeError.unsupportedLayout("backup partition array location is invalid")
    }
    let backupEntriesBytes = try readBytes(
      handle, offset: backupEntriesOffset, count: entriesCount)
    guard CRC32.checksum(backupEntriesBytes) == backup.partitionEntryArrayCRC32 else {
      throw DiskResizeError.unsupportedLayout("backup partition array CRC mismatch")
    }
    guard backupEntriesBytes == entriesBytes else {
      throw DiskResizeError.unsupportedLayout("primary and backup partition arrays differ")
    }

    return GPTTable(header: header, entries: entries)
  }

  // MARK: Write

  /// Serializes the 128-entry array into its on-disk byte blob.
  func serializedEntries() -> [UInt8] {
    var blob = [UInt8]()
    let count = Int(header.numberOfPartitionEntries)
    blob.reserveCapacity(count * GPTPartitionEntry.byteSize)
    for i in 0..<count { blob.append(contentsOf: entries[i].serialized()) }
    return blob
  }

  /// Writes both GPT copies and the protective MBR using crash-safe ordering:
  /// backup entries → backup header → fullsync → primary entries → primary
  /// header → protective MBR → fullsync. `header.alternateLBA` /
  /// `header.lastUsableLBA` are taken as the source of truth for total size.
  func write(to handle: FileHandle) throws {
    let ss = UInt64(GPTTable.sectorSize)
    let total = totalSectors
    let entriesBlob = serializedEntries()
    let entriesCRC = CRC32.checksum(entriesBlob)
    let arraySectors = UInt64(entriesBlob.count) / ss

    var primary = header
    primary.myLBA = 1
    primary.alternateLBA = total - 1
    primary.partitionEntryLBA = 2
    primary.partitionEntryArrayCRC32 = entriesCRC

    var backup = primary
    backup.myLBA = total - 1
    backup.alternateLBA = 1
    backup.partitionEntryLBA = total - 1 - arraySectors

    let entriesData = Data(entriesBlob)

    // Backup copy first.
    try handle.seek(toOffset: backup.partitionEntryLBA * ss)
    try handle.write(contentsOf: entriesData)
    try handle.seek(toOffset: (total - 1) * ss)
    try handle.write(contentsOf: Data(backup.serializedSector()))
    try GPT.fullSync(handle)

    // Primary copy.
    try handle.seek(toOffset: 2 * ss)
    try handle.write(contentsOf: entriesData)
    try handle.seek(toOffset: 1 * ss)
    try handle.write(contentsOf: Data(primary.serializedSector()))

    try updateProtectiveMBR(handle: handle, totalSectors: total)
    try GPT.fullSync(handle)
  }

  /// Updates the protective MBR's 0xEE record size-in-LBA field, preserving
  /// boot code and the 0x55AA signature. Clamps to 0xFFFFFFFF per spec.
  private func updateProtectiveMBR(handle: FileHandle, totalSectors: UInt64) throws {
    var mbr = try GPTTable.readSector(handle, lba: 0)
    let sizeInLBA = UInt32(min(totalSectors - 1, UInt64(UInt32.max)))
    var updated = false
    for record in 0..<4 {
      let base = 446 + record * 16
      if mbr[base + 4] == 0xEE {
        putLE32(sizeInLBA, into: &mbr, at: base + 12)  // size-in-LBA field
        updated = true
      }
    }
    if !updated {
      // Synthesize a canonical protective MBR record if none present.
      let base = 446
      mbr[base] = 0x00
      mbr[base + 1] = 0x00
      mbr[base + 2] = 0x02
      mbr[base + 3] = 0x00
      mbr[base + 4] = 0xEE
      mbr[base + 5] = 0xFF
      mbr[base + 6] = 0xFF
      mbr[base + 7] = 0xFF
      putLE32(1, into: &mbr, at: base + 8)
      putLE32(sizeInLBA, into: &mbr, at: base + 12)
    }
    mbr[510] = 0x55
    mbr[511] = 0xAA
    try handle.seek(toOffset: 0)
    try handle.write(contentsOf: Data(mbr))
  }

  // MARK: Low-level IO

  static func readSector(_ handle: FileHandle, lba: UInt64) throws -> [UInt8] {
    try readBytes(handle, offset: lba * UInt64(sectorSize), count: sectorSize)
  }

  static func readBytes(_ handle: FileHandle, offset: UInt64, count: Int) throws -> [UInt8] {
    try handle.seek(toOffset: offset)
    guard let data = try handle.read(upToCount: count), data.count == count else {
      throw DiskResizeError.unsupportedLayout(
        "could not read \(count) bytes at offset \(offset)")
    }
    return [UInt8](data)
  }
}

// MARK: - Sync

enum GPT {
  static let sectorSize = 512

  static func fullSync(_ handle: FileHandle) throws {
    guard fcntl(handle.fileDescriptor, F_FULLFSYNC) == 0 else {
      let details = String(cString: strerror(errno))
      throw DiskResizeError.commandFailed(command: "fcntl(F_FULLFSYNC)", output: details)
    }
  }
}
