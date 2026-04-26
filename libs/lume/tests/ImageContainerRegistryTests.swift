import Foundation
import Testing

@testable import lume

@Test("parameterized OCI tar disk layers are recognized as raw disk parts")
func testParameterizedTarDiskLayerIsRecognizedAsRawDiskPart() throws {
    let layer = Layer(
        mediaType: "application/vnd.oci.image.layer.v1.tar;part.number=2;part.total=41",
        digest: "sha256:part2",
        size: 512,
        annotations: ["org.opencontainers.image.title": "disk.img.part.ab"]
    )

    let diskPart = OCIMediaType.rawDiskPartInfo(for: layer)

    #expect(diskPart?.partNumber == 2)
    #expect(diskPart?.totalParts == 41)
}

@Test("non-disk OCI tar layers are not treated as disk parts")
func testNonDiskTarLayerIsNotRecognizedAsRawDiskPart() throws {
    let layer = Layer(
        mediaType: "application/vnd.oci.image.layer.v1.tar;part.number=1;part.total=1",
        digest: "sha256:config",
        size: 128,
        annotations: ["org.opencontainers.image.title": "metadata.tar"]
    )

    #expect(OCIMediaType.rawDiskPartInfo(for: layer) == nil)
}

@Test("disk part collector preserves explicit part ordering")
func testDiskPartsCollectorPreservesExplicitPartOrdering() async {
    let collector = DiskPartsCollector()
    let tempDir = FileManager.default.temporaryDirectory

    await collector.addPart(
        partNumber: 10,
        url: tempDir.appendingPathComponent("disk.img.part.10"),
        compression: .raw
    )
    await collector.addPart(
        partNumber: 2,
        url: tempDir.appendingPathComponent("disk.img.part.2"),
        compression: .raw
    )
    await collector.addPart(
        partNumber: 1,
        url: tempDir.appendingPathComponent("disk.img.part.1"),
        compression: .raw
    )

    let parts = await collector.getSortedParts()

    #expect(parts.map(\.partNumber) == [1, 2, 10])
    #expect(parts.map(\.compression) == [.raw, .raw, .raw])
}

@Test("raw disk part writer copies bytes without decompression")
func testRawDiskPartWriterCopiesBytesWithoutDecompression() throws {
    let tempDir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: tempDir) }

    let source = tempDir.appendingPathComponent("disk.img.part.1")
    let destination = tempDir.appendingPathComponent("disk.img")
    try Data([0xde, 0xad, 0xbe, 0xef]).write(to: source)
    _ = FileManager.default.createFile(atPath: destination.path, contents: nil)

    let handle = try FileHandle(forWritingTo: destination)

    let writtenBytes = try DiskPartWriter.writeRawChunk(
        inputPath: source.path,
        outputHandle: handle,
        startOffset: 2
    )

    try handle.close()
    let output = try Data(contentsOf: destination)

    #expect(writtenBytes == 4)
    #expect(output == Data([0x00, 0x00, 0xde, 0xad, 0xbe, 0xef]))
}
