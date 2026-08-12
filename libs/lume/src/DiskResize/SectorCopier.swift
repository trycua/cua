import CryptoKit
import Darwin
import Foundation

/// Positioned, sparse-aware copy/hash primitives used to relocate the recovery
/// partition byte-for-byte and to verify it afterwards. All operations use
/// `pread`/`pwrite` at explicit offsets so they never disturb (or depend on) a
/// shared file offset.
enum SectorCopier {
  static let sectorSize: UInt64 = 512
  static let chunkSize = 8 * 1024 * 1024  // 8 MiB

  /// Copies `lengthSectors` from `srcLBA` to `dstLBA`, skipping all-zero
  /// chunks so the destination stays sparse, and returns the SHA-256 of the
  /// full logical source content (holes hashed as zeros). The source and
  /// destination ranges must not overlap.
  @discardableResult
  static func copyRange(fd: Int32, srcLBA: UInt64, dstLBA: UInt64, lengthSectors: UInt64) throws
    -> Data
  {
    let byteLen = lengthSectors * sectorSize
    let srcStart = srcLBA * sectorSize
    let dstStart = dstLBA * sectorSize
    precondition(
      disjoint(aStart: srcStart, bStart: dstStart, length: byteLen),
      "SectorCopier.copyRange requires non-overlapping ranges")

    var hasher = SHA256()
    var buf = [UInt8](repeating: 0, count: chunkSize)
    var offset: UInt64 = 0
    while offset < byteLen {
      let n = Int(min(UInt64(chunkSize), byteLen - offset))
      let read = try preadExact(fd: fd, into: &buf, count: n, offset: srcStart + offset)
      let chunk = Array(buf[0..<read])
      hasher.update(data: Data(chunk))
      if chunk.contains(where: { $0 != 0 }) {
        try pwriteExact(fd: fd, bytes: chunk, offset: dstStart + offset)
      }
      offset += UInt64(read)
    }
    return Data(hasher.finalize())
  }

  /// SHA-256 of the logical content of a sector range (holes read as zeros).
  static func hashRange(fd: Int32, startLBA: UInt64, lengthSectors: UInt64) throws -> Data {
    let byteLen = lengthSectors * sectorSize
    let start = startLBA * sectorSize
    var hasher = SHA256()
    var buf = [UInt8](repeating: 0, count: chunkSize)
    var offset: UInt64 = 0
    while offset < byteLen {
      let n = Int(min(UInt64(chunkSize), byteLen - offset))
      let read = try preadExact(fd: fd, into: &buf, count: n, offset: start + offset)
      hasher.update(data: Data(buf[0..<read]))
      offset += UInt64(read)
    }
    return Data(hasher.finalize())
  }

  /// Allocated (non-hole) bytes inside a sector range, via a SEEK_DATA walk.
  static func allocatedBytes(fd: Int32, startLBA: UInt64, lengthSectors: UInt64) -> UInt64 {
    let start = startLBA * sectorSize
    let end = start + lengthSectors * sectorSize
    var pos = start
    var allocated: UInt64 = 0
    while pos < end {
      let dataStart = lseek(fd, off_t(pos), SEEK_DATA)
      if dataStart < 0 || UInt64(dataStart) >= end { break }
      let holeStart = lseek(fd, dataStart, SEEK_HOLE)
      let regionEnd = holeStart < 0 ? end : min(UInt64(holeStart), end)
      allocated += regionEnd - UInt64(dataStart)
      pos = regionEnd
    }
    return allocated
  }

  /// Punches a hole over a sector range (best-effort; keeps the image sparse).
  static func punchHole(fd: Int32, startLBA: UInt64, lengthSectors: UInt64) {
    var punch = fpunchhole_t(
      fp_flags: 0,
      reserved: 0,
      fp_offset: off_t(startLBA * sectorSize),
      fp_length: off_t(lengthSectors * sectorSize))
    _ = fcntl(fd, F_PUNCHHOLE, &punch)
  }

  // MARK: - Positioned IO

  private static func preadExact(fd: Int32, into buf: inout [UInt8], count: Int, offset: UInt64)
    throws -> Int
  {
    var total = 0
    try buf.withUnsafeMutableBytes { raw in
      let base = raw.baseAddress!
      while total < count {
        let n = pread(fd, base + total, count - total, off_t(offset) + off_t(total))
        if n < 0 {
          throw DiskResizeError.commandFailed(
            command: "pread", output: String(cString: strerror(errno)))
        }
        if n == 0 { break }  // EOF
        total += n
      }
    }
    return total
  }

  private static func pwriteExact(fd: Int32, bytes: [UInt8], offset: UInt64) throws {
    try bytes.withUnsafeBytes { raw in
      let base = raw.baseAddress!
      var total = 0
      while total < bytes.count {
        let n = pwrite(fd, base + total, bytes.count - total, off_t(offset) + off_t(total))
        if n <= 0 {
          throw DiskResizeError.commandFailed(
            command: "pwrite", output: String(cString: strerror(errno)))
        }
        total += n
      }
    }
  }

  private static func disjoint(aStart: UInt64, bStart: UInt64, length: UInt64) -> Bool {
    let aEnd = aStart + length
    let bEnd = bStart + length
    return aEnd <= bStart || bEnd <= aStart
  }
}
