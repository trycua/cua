import Foundation
import Testing

@testable import lume

struct OCIChunkReuseTests {
  private func makeLayer(digest: String, part: Int, offset: UInt64) -> Layer {
    Layer(
      mediaType: OCIMediaType.disk,
      digest: digest,
      size: 512,
      annotations: [
        OCIAnnotation.partNumber: String(part),
        OCIAnnotation.partOffset: String(offset),
      ])
  }

  private func gzip(_ data: Data, to destination: URL) throws {
    let source = destination.deletingPathExtension().appendingPathExtension("raw-source")
    try data.write(to: source)
    defer { try? FileManager.default.removeItem(at: source) }

    FileManager.default.createFile(atPath: destination.path, contents: nil)
    let output = try FileHandle(forWritingTo: destination)
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/usr/bin/gzip")
    process.arguments = ["-c", source.path]
    process.standardOutput = output
    try process.run()
    process.waitUntilExit()
    try output.close()
    #expect(process.terminationStatus == 0)
  }

  @Test("logical OCI chunks are grouped by digest without losing offsets")
  func groupsRepeatedDigests() {
    let layers = [
      makeLayer(digest: "sha256:nonzero", part: 0, offset: 0),
      makeLayer(digest: "sha256:zero", part: 1, offset: 4_194_304),
      makeLayer(digest: "sha256:nonzero", part: 2, offset: 8_388_608),
      makeLayer(digest: "sha256:zero", part: 3, offset: 12_582_912),
    ]

    let groups = groupOCIChunksByDigest(layers)

    #expect(groups.count == 2)
    #expect(groups[0].layer.digest == "sha256:nonzero")
    #expect(
      groups[0].references
        == [
          OCIChunkReference(index: 0, offset: 0),
          OCIChunkReference(index: 2, offset: 8_388_608),
        ])
    #expect(groups[1].layer.digest == "sha256:zero")
    #expect(
      groups[1].references
        == [
          OCIChunkReference(index: 1, offset: 4_194_304),
          OCIChunkReference(index: 3, offset: 12_582_912),
        ])
    #expect(uniqueLayersByDigest(layers).map(\.digest) == ["sha256:nonzero", "sha256:zero"])
  }

  @Test("one compressed zero or nonzero blob can populate multiple logical parts")
  func reusesCompressedBlobAtMultipleOffsets() throws {
    let tempDir = FileManager.default.temporaryDirectory
      .appendingPathComponent("lume-oci-chunk-reuse-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: tempDir) }

    let nonzero = Data("reused-nonzero-chunk".utf8)
    let zero = Data(count: 4 * 1024 * 1024)
    let nonzeroGzip = tempDir.appendingPathComponent("nonzero.gz")
    let zeroGzip = tempDir.appendingPathComponent("zero.gz")
    try gzip(nonzero, to: nonzeroGzip)
    try gzip(zero, to: zeroGzip)

    let outputURL = tempDir.appendingPathComponent("disk.img")
    FileManager.default.createFile(atPath: outputURL.path, contents: nil)
    let output = try FileHandle(forWritingTo: outputURL)
    defer { try? output.close() }
    try output.truncate(atOffset: 12 * 1024 * 1024)

    #expect(
      try gunzipChunkAndWriteSparse(
        inputPath: nonzeroGzip, outputHandle: output, startOffset: 0)
        == UInt64(nonzero.count))
    #expect(
      try gunzipChunkAndWriteSparse(
        inputPath: nonzeroGzip, outputHandle: output, startOffset: 64)
        == UInt64(nonzero.count))
    #expect(
      try gunzipChunkAndWriteSparse(
        inputPath: zeroGzip, outputHandle: output, startOffset: 4 * 1024 * 1024)
        == UInt64(zero.count))
    #expect(
      try gunzipChunkAndWriteSparse(
        inputPath: zeroGzip, outputHandle: output, startOffset: 8 * 1024 * 1024)
        == UInt64(zero.count))

    let reader = try FileHandle(forReadingFrom: outputURL)
    defer { try? reader.close() }
    try reader.seek(toOffset: 0)
    #expect(reader.readData(ofLength: nonzero.count) == nonzero)
    try reader.seek(toOffset: 64)
    #expect(reader.readData(ofLength: nonzero.count) == nonzero)
    try reader.seek(toOffset: 4 * 1024 * 1024)
    #expect(reader.readData(ofLength: 4096) == Data(count: 4096))
    try reader.seek(toOffset: 8 * 1024 * 1024)
    #expect(reader.readData(ofLength: 4096) == Data(count: 4096))
  }

  @Test("a missing compressed chunk fails closed")
  func missingCompressedChunkThrows() throws {
    let tempDir = FileManager.default.temporaryDirectory
      .appendingPathComponent("lume-oci-missing-chunk-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: tempDir) }

    let outputURL = tempDir.appendingPathComponent("disk.img")
    FileManager.default.createFile(atPath: outputURL.path, contents: nil)
    let output = try FileHandle(forWritingTo: outputURL)
    defer { try? output.close() }
    let missing = tempDir.appendingPathComponent("missing.gz")

    do {
      _ = try gunzipChunkAndWriteSparse(
        inputPath: missing, outputHandle: output, startOffset: 0)
      Issue.record("Expected a missing compressed chunk to fail the pull")
    } catch PullError.layerDownloadFailed(let filename) {
      #expect(filename == "missing.gz")
    } catch {
      Issue.record("Unexpected error: \(error)")
    }
  }
}
