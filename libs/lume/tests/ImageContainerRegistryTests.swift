import Foundation
import Testing

@testable import lume

private func tarLayer(_ parameters: String, title: String? = "disk.img.part.ab") -> Layer {
    Layer(
        mediaType: "application/vnd.oci.image.layer.v1.tar\(parameters)",
        digest: "sha256:\(UUID().uuidString)", size: 1,
        annotations: title.map { ["org.opencontainers.image.title": $0] })
}

private func rawPart(_ number: String, of total: String) -> Layer {
    tarLayer(";part.number=\(number);part.total=\(total)")
}

private func manifest(_ layers: [Layer]) -> Manifest {
    Manifest(
        layers: layers, config: nil, mediaType: "application/vnd.oci.image.manifest.v1+json",
        schemaVersion: 2, annotations: nil)
}

@Test("parameterized OCI tar disk layers are recognized as raw disk parts",
      arguments: [
          "application/vnd.oci.image.layer.v1.tar;part.number=2;part.total=41",
          "Application/Vnd.Oci.Image.Layer.V1.Tar; Part.Number = 2 ; PART.TOTAL = 41",
      ])
func recognizesRawDiskPart(mediaType: String) {
    let layer = Layer(
        mediaType: mediaType, digest: "sha256:part2", size: 512,
        annotations: ["org.opencontainers.image.title": "disk.img.part.ab"])
    let part = OCIMediaType.rawDiskPart(layer)
    #expect(part?.number == 2)
    #expect(part?.total == 41)
}

@Test("layers without a disk part title or tar base type are not raw disk parts",
      arguments: [
          tarLayer(";part.number=1;part.total=1", title: "metadata.tar"),
          tarLayer(";part.number=1;part.total=1", title: nil),
          tarLayer("+gzip;part.number=1;part.total=1"),
      ])
func ignoresNonDiskLayers(layer: Layer) {
    #expect(OCIMediaType.rawDiskPart(layer)?.number == nil)
}

@Test("complete raw disk part sets and manifests without raw parts are accepted")
func acceptsValidRawDiskParts() throws {
    try OCIMediaType.validateRawDiskParts(manifest([rawPart("2", of: "2"), rawPart("1", of: "2")]))
    try OCIMediaType.validateRawDiskParts(manifest([
        Layer(mediaType: "application/octet-stream+lz4", digest: "sha256:lz4", size: 1, annotations: nil)
    ]))
}

@Test("invalid raw disk part sets are rejected as reassembly failures",
      arguments: [
          [rawPart("1", of: "2"), rawPart("2", of: "3")],
          [rawPart("1", of: "2"), rawPart("1", of: "2")],
          [rawPart("1", of: "3"), rawPart("2", of: "3")],
          [rawPart("2", of: "2"), rawPart("3", of: "2")],
          [rawPart("2", of: "1")],
          [rawPart("0", of: "1")],
          [rawPart("-1", of: "1")],
          [rawPart("x", of: "1")],
          [rawPart("1", of: "99999999999999999999")],
          [tarLayer(";part.total=1")],
          [tarLayer("")],
          [
              rawPart("1", of: "1"),
              Layer(mediaType: "application/octet-stream+lz4", digest: "sha256:lz4", size: 1, annotations: nil),
          ],
      ])
func rejectsInvalidRawDiskParts(layers: [Layer]) {
    do {
        try OCIMediaType.validateRawDiskParts(manifest(layers))
        Issue.record("expected reassemblyFailed")
    } catch PullError.reassemblyFailed {
    } catch {
        Issue.record("expected reassemblyFailed, got \(error)")
    }
}
