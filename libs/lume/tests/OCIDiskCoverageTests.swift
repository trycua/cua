import Foundation
import Testing

@testable import lume

/// A chunked push that lost disk chunks published a manifest whose layers
/// leave holes (ghcr.io/trycua/macos:26-20260927-9a3828d lacks parts 31, 63
/// and 64); pulled, the holes read as zeros and the guest booted into
/// recoveryOS. Pull refuses such a manifest and push never writes one.
struct OCIDiskCoverageTests {
  private let chunk: UInt64 = 512

  private func manifest(parts: [Int], total: UInt64) -> Manifest {
    let layers = parts.map { part in
      Layer(
        mediaType: OCIMediaType.disk,
        digest: "sha256:\(part)",
        size: 10,
        annotations: [
          OCIAnnotation.partNumber: String(part),
          OCIAnnotation.partOffset: String(UInt64(part) * chunk),
          "org.trycua.lume.content.uncompressed-size": String(chunk),
        ])
    }
    return Manifest(
      layers: layers, config: nil, mediaType: "application/vnd.oci.image.manifest.v1+json",
      schemaVersion: 2,
      annotations: ["org.trycua.lume.total-uncompressed-size": String(total)])
  }

  @Test func contiguousChunksCoverTheDisk() throws {
    #expect(diskChunkCoverageGap([(0, 512), (512, 512), (1024, 100)], total: 1124) == nil)
    try ImageContainerRegistry.checkDiskCoverage(manifest(parts: [0, 1, 2, 3], total: 4 * chunk))
  }

  @Test func aMissingChunkIsAGap() {
    #expect(
      diskChunkCoverageGap([(0, 512), (1024, 512)], total: 1536)
        == "bytes 512..<1024 are in no disk layer")
    #expect(
      diskChunkCoverageGap([(0, 512)], total: 1024) == "bytes 512..<1024 are in no disk layer")
    #expect(diskChunkCoverageGap([(0, 512), (256, 512)], total: 768) != nil)
    #expect(diskChunkCoverageGap([(0, 2048)], total: 1024) != nil)
  }

  @Test func pullRefusesAManifestWithMissingParts() {
    #expect(throws: PullError.self) {
      try ImageContainerRegistry.checkDiskCoverage(manifest(parts: [0, 1, 3], total: 4 * chunk))
    }
  }

  @Test func manifestsWithoutOffsetsAreNotChecked() throws {
    let legacy = Manifest(
      layers: [Layer(mediaType: OCIMediaType.disk, digest: "sha256:a", size: 1, annotations: nil)],
      config: nil, mediaType: "application/vnd.oci.image.manifest.v1+json", schemaVersion: 2,
      annotations: ["org.trycua.lume.total-uncompressed-size": "4096"])
    try ImageContainerRegistry.checkDiskCoverage(legacy)
  }
}
