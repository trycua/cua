import Foundation
import Testing

@testable import lume

// MARK: - GUID / CRC primitives

@Suite("GPT primitives")
struct GPTPrimitiveTests {
  @Test("Apple type GUIDs round-trip string ↔ raw mixed-endian bytes")
  func guidRoundTrip() {
    #expect(GPTGUID.appleAPFSISC.description == "69646961-6700-11AA-AA11-00306543ECAC")
    #expect(GPTGUID.appleAPFS.description == "7C3457EF-0000-11AA-AA11-00306543ECAC")
    #expect(GPTGUID.appleAPFSRecovery.description == "52637672-7900-11AA-AA11-00306543ECAC")
    // First on-disk bytes of ISC are the little-endian Data1 (mixed-endian).
    #expect(Array(GPTGUID.appleAPFSISC.bytes.prefix(4)) == [0x61, 0x69, 0x64, 0x69])
  }

  @Test("CRC32 matches the known zlib checksum of 'The quick brown fox...'")
  func crc32KnownVector() {
    let input = Array("The quick brown fox jumps over the lazy dog".utf8)
    #expect(CRC32.checksum(input) == 0x414F_A339)
  }
}

// MARK: - Round-trip

@Suite("GPT read/write round-trip")
struct GPTRoundTripTests {
  @Test("read → write reproduces LBA 0–33 and the trailing 33 sectors byte-for-byte")
  func roundTrip() throws {
    let (url, _) = try GPTFixtures.makeDisk()
    defer { GPTFixtures.remove(url) }

    let ss = Int(GPTTable.sectorSize)
    let original = try Data(contentsOf: url)
    let total = Int(original.count) / ss

    // Re-read and re-write into a copy.
    let copy = url.appendingPathExtension("copy")
    try original.write(to: copy)
    defer { GPTFixtures.remove(copy) }

    let readHandle = try FileHandle(forReadingFrom: copy)
    let table = try GPTTable.read(from: readHandle, fileSizeBytes: UInt64(original.count))
    try readHandle.close()

    let writeHandle = try FileHandle(forUpdating: copy)  // write() reads the MBR back
    try table.write(to: writeHandle)
    try writeHandle.close()

    let rewritten = try Data(contentsOf: copy)
    // First 34 sectors (MBR + primary header + primary entry array).
    #expect(original.prefix(34 * ss) == rewritten.prefix(34 * ss))
    // Last 33 sectors (backup entry array + backup header).
    #expect(original.suffix(33 * ss) == rewritten.suffix(33 * ss))
    #expect(total == Int(GPTFixtures.defaultTotalSectors))
  }

  @Test("fingerprint accepts the standard fixture and identifies all three partitions")
  func fingerprintHappyPath() throws {
    let (url, _) = try GPTFixtures.makeDisk()
    defer { GPTFixtures.remove(url) }
    let handle = try FileHandle(forReadingFrom: url)
    defer { try? handle.close() }
    let table = try GPTTable.read(from: handle, fileSizeBytes: GPTFixtures.fileSize(url))
    let layout = try MacDiskLayout.fingerprint(table, fileSizeBytes: GPTFixtures.fileSize(url))
    #expect(layout.isc.typeGUID == .appleAPFSISC)
    #expect(layout.main.typeGUID == .appleAPFS)
    #expect(layout.recovery.typeGUID == .appleAPFSRecovery)
    #expect(layout.recovery.startingLBA == 123_032)
    #expect(layout.recoveryLength == 8_000)
  }

  @Test("fingerprint accepts an already-truncated (grown) file as a repair starting state")
  func fingerprintRepairCase() throws {
    let (url, _) = try GPTFixtures.makeDisk()
    defer { GPTFixtures.remove(url) }
    // Simulate old `lume set` ftruncate: file bigger than the GPT describes.
    let handle = try FileHandle(forWritingTo: url)
    let grown = GPTFixtures.defaultTotalSectors * GPTFixtures.sectorSize + 10 * 1024 * 1024
    try handle.truncate(atOffset: grown)
    try handle.close()

    let readHandle = try FileHandle(forReadingFrom: url)
    defer { try? readHandle.close() }
    let table = try GPTTable.read(from: readHandle, fileSizeBytes: grown)
    #expect(throws: Never.self) {
      _ = try MacDiskLayout.fingerprint(table, fileSizeBytes: grown)
    }
  }
}

// MARK: - Parser rejection

@Suite("GPT parser rejection")
struct GPTParserRejectionTests {
  private func corrupt(_ url: URL, atOffset offset: UInt64, xor: UInt8) throws {
    let handle = try FileHandle(forUpdating: url)
    defer { try? handle.close() }
    try handle.seek(toOffset: offset)
    let byte = try handle.read(upToCount: 1) ?? Data([0])
    try handle.seek(toOffset: offset)
    try handle.write(contentsOf: Data([byte[byte.startIndex] ^ xor]))
  }

  @Test("bad primary header CRC is rejected")
  func badHeaderCRC() throws {
    let (url, _) = try GPTFixtures.makeDisk()
    defer { GPTFixtures.remove(url) }
    // Flip a byte in the header body (myLBA area) without fixing the CRC.
    try corrupt(url, atOffset: 512 + 40, xor: 0xFF)  // firstUsableLBA byte
    let handle = try FileHandle(forReadingFrom: url)
    defer { try? handle.close() }
    #expect(throws: DiskResizeError.self) {
      _ = try GPTTable.read(from: handle, fileSizeBytes: GPTFixtures.fileSize(url))
    }
  }

  @Test("bad partition-array CRC is rejected")
  func badEntriesCRC() throws {
    let (url, _) = try GPTFixtures.makeDisk()
    defer { GPTFixtures.remove(url) }
    // Corrupt a byte inside the primary entry array (LBA 2).
    try corrupt(url, atOffset: 2 * 512 + 40, xor: 0x01)
    let handle = try FileHandle(forReadingFrom: url)
    defer { try? handle.close() }
    #expect(throws: DiskResizeError.self) {
      _ = try GPTTable.read(from: handle, fileSizeBytes: GPTFixtures.fileSize(url))
    }
  }

  @Test("bad backup partition-array CRC is rejected")
  func badBackupEntriesCRC() throws {
    let (url, _) = try GPTFixtures.makeDisk()
    defer { GPTFixtures.remove(url) }
    let handle = try FileHandle(forReadingFrom: url)
    let table = try GPTTable.read(from: handle, fileSizeBytes: GPTFixtures.fileSize(url))
    try handle.close()
    let backupArrayLBA = table.header.alternateLBA - 32
    try corrupt(url, atOffset: backupArrayLBA * 512 + 40, xor: 0x01)
    let readHandle = try FileHandle(forReadingFrom: url)
    defer { try? readHandle.close() }
    #expect(throws: DiskResizeError.self) {
      _ = try GPTTable.read(from: readHandle, fileSizeBytes: GPTFixtures.fileSize(url))
    }
  }

  @Test("missing GPT signature is rejected")
  func badSignature() throws {
    let (url, _) = try GPTFixtures.makeDisk()
    defer { GPTFixtures.remove(url) }
    try corrupt(url, atOffset: 512, xor: 0xFF)  // 'E' of "EFI PART"
    let handle = try FileHandle(forReadingFrom: url)
    defer { try? handle.close() }
    #expect(throws: DiskResizeError.self) {
      _ = try GPTTable.read(from: handle, fileSizeBytes: GPTFixtures.fileSize(url))
    }
  }

  @Test("4096-byte-sector image reports unsupportedSectorSize")
  func fourKSector() throws {
    let url = FileManager.default.temporaryDirectory
      .appendingPathComponent("lume-4k-\(UUID().uuidString).img")
    FileManager.default.createFile(atPath: url.path, contents: nil)
    let handle = try FileHandle(forWritingTo: url)
    try handle.truncate(atOffset: 64 * 1024 * 1024)
    // Put "EFI PART" at offset 4096 (LBA 1 for a 4K-sector disk), nothing at 512.
    try handle.seek(toOffset: 4096)
    try handle.write(contentsOf: Data("EFI PART".utf8))
    try handle.close()
    defer { GPTFixtures.remove(url) }

    let readHandle = try FileHandle(forReadingFrom: url)
    defer { try? readHandle.close() }
    var thrown: DiskResizeError?
    #expect(throws: DiskResizeError.self) {
      do {
        _ = try GPTTable.read(from: readHandle, fileSizeBytes: GPTFixtures.fileSize(url))
      } catch let error as DiskResizeError {
        thrown = error
        throw error
      }
    }
    if case .unsupportedSectorSize = thrown {
    } else {
      Issue.record("expected .unsupportedSectorSize, got \(String(describing: thrown))")
    }
  }

  @Test("file smaller than the GPT-described size (missing backup) is rejected")
  func truncatedBelowGPT() throws {
    let (url, _) = try GPTFixtures.makeDisk()
    defer { GPTFixtures.remove(url) }
    let handle = try FileHandle(forWritingTo: url)
    // Truncate so the backup header no longer exists.
    try handle.truncate(atOffset: (GPTFixtures.defaultTotalSectors - 100) * 512)
    try handle.close()
    let readHandle = try FileHandle(forReadingFrom: url)
    defer { try? readHandle.close() }
    #expect(throws: DiskResizeError.self) {
      _ = try GPTTable.read(
        from: readHandle, fileSizeBytes: (GPTFixtures.defaultTotalSectors - 100) * 512)
    }
  }
}

// MARK: - Fingerprint rejection

@Suite("MacDiskLayout fingerprint rejection")
struct FingerprintRejectionTests {
  private func fingerprint(
    _ layout: GPTFixtures.Layout, mutate: (inout GPTTable) -> Void = { _ in }
  )
    throws -> MacDiskLayout
  {
    let (url, _) = try GPTFixtures.makeDisk(layout: layout)
    defer { GPTFixtures.remove(url) }
    let handle = try FileHandle(forReadingFrom: url)
    defer { try? handle.close() }
    var table = try GPTTable.read(from: handle, fileSizeBytes: GPTFixtures.fileSize(url))
    mutate(&table)
    return try MacDiskLayout.fingerprint(table, fileSizeBytes: GPTFixtures.fileSize(url))
  }

  @Test("only two populated partitions is rejected")
  func twoPartitions() {
    #expect(throws: DiskResizeError.self) {
      _ = try fingerprint(.init()) { table in
        table.entries[2] = GPTFixtures.emptyEntry()
      }
    }
  }

  @Test("a populated 4th entry is rejected")
  func fourthEntry() {
    #expect(throws: DiskResizeError.self) {
      _ = try fingerprint(.init()) { table in
        table.entries[3] = table.entries[2]
      }
    }
  }

  @Test("wrong type order (main and recovery swapped) is rejected")
  func wrongTypeOrder() {
    #expect(throws: DiskResizeError.self) {
      _ = try fingerprint(.init()) { table in
        table.entries.swapAt(1, 2)
      }
    }
  }

  @Test("recovery not last (extends past LastUsableLBA) is rejected")
  func recoveryPastLastUsable() {
    var layout = GPTFixtures.Layout()
    layout.recoveryEnd = 131_050  // > lastUsable 131038
    #expect(throws: DiskResizeError.self) { _ = try fingerprint(layout) }
  }

  @Test("recovery ending more than 7 sectors before LastUsableLBA is rejected")
  func recoveryTooEarly() {
    var layout = GPTFixtures.Layout()
    layout.recoveryStart = 123_008
    layout.recoveryEnd = 131_007  // slack = 131038 - 131007 = 31 > 7
    #expect(throws: DiskResizeError.self) { _ = try fingerprint(layout) }
  }

  @Test("unaligned recovery start is rejected")
  func unalignedRecovery() {
    var layout = GPTFixtures.Layout()
    layout.recoveryStart = 123_033  // not a multiple of 8
    layout.recoveryEnd = 131_032
    #expect(throws: DiskResizeError.self) { _ = try fingerprint(layout) }
  }
}

// MARK: - SectorCopier

@Suite("SectorCopier")
struct SectorCopierTests {
  @Test("copyRange relocates recovery bytes and the hash matches the source range")
  func copyAndHash() throws {
    let (url, _) = try GPTFixtures.makeDisk()
    defer { GPTFixtures.remove(url) }
    // Grow the file so there is room past the recovery region to copy into.
    let handle = try FileHandle(forUpdating: url)
    defer { try? handle.close() }
    let newTotal = GPTFixtures.defaultTotalSectors + 20_000
    try handle.truncate(atOffset: newTotal * 512)

    let srcStart: UInt64 = 123_032
    let len: UInt64 = 8_000
    let dstStart: UInt64 = 140_000  // well past the source, non-overlapping

    let srcHash = try SectorCopier.hashRange(
      fd: handle.fileDescriptor, startLBA: srcStart, lengthSectors: len)
    let copyHash = try SectorCopier.copyRange(
      fd: handle.fileDescriptor, srcLBA: srcStart, dstLBA: dstStart, lengthSectors: len)
    let dstHash = try SectorCopier.hashRange(
      fd: handle.fileDescriptor, startLBA: dstStart, lengthSectors: len)

    #expect(srcHash == copyHash)
    #expect(srcHash == dstHash)
  }

  @Test("punchHole reduces allocated bytes over a region")
  func punchReducesAllocation() throws {
    let (url, _) = try GPTFixtures.makeDisk()
    defer { GPTFixtures.remove(url) }
    let handle = try FileHandle(forUpdating: url)
    defer { try? handle.close() }
    let fd = handle.fileDescriptor
    let start: UInt64 = 123_032
    let len: UInt64 = 8_000
    let before = SectorCopier.allocatedBytes(fd: fd, startLBA: start, lengthSectors: len)
    #expect(before > 0)
    SectorCopier.punchHole(fd: fd, startLBA: start, lengthSectors: len)
    let after = SectorCopier.allocatedBytes(fd: fd, startLBA: start, lengthSectors: len)
    #expect(after < before)
  }
}
