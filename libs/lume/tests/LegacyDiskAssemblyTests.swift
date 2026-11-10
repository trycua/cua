import Foundation
import Testing

@testable import lume

struct LegacyDiskAssemblyTests {
    @Test("legacy LZ4 assembly preserves disk bytes and cached output",
          arguments: [false, true], [false, true])
    func preservesBytes(cachingEnabled: Bool, trailingZeroChunk: Bool) async throws {
        let first = Data((0..<64).map { UInt8($0 + 1) })
        let last = trailingZeroChunk
            ? Data(count: 4 * 1024 * 1024)
            : Data((0..<73).map { UInt8(255 - $0) })
        let parts = try [first, last].enumerated().map { index, bytes in
            let compressed = try (bytes as NSData).compressed(using: .lz4) as Data
            let layer = Layer(
                mediaType: "application/octet-stream+lz4", digest: "sha256:part-\(index)",
                size: compressed.count, annotations: nil)
            return (layer, compressed)
        }
        try await assertAssembles(
            parts: parts, expected: first + last, cachingEnabled: cachingEnabled)
    }

    @Test("raw OCI tar parts assemble in part.number order",
          arguments: [false, true], ["single", "multi", "trailingZero"])
    func assemblesRawTarParts(cachingEnabled: Bool, layout: String) async throws {
        let first = Data((0..<64).map { UInt8($0 + 1) })
        let middle = Data((0..<73).map { UInt8(255 - $0) })
        let payloads = [
            "single": [first],
            "multi": [first, middle, Data(repeating: 7, count: 5)],
            "trailingZero": [first, middle, Data(count: 4 * 1024 * 1024)],
        ][layout]!
        let titleSuffixes = ["zz", "yy", "xx"]
        let parts = payloads.enumerated().reversed().map { index, bytes in
            let layer = Layer(
                mediaType: "application/vnd.oci.image.layer.v1.tar;part.number=\(index + 1);part.total=\(payloads.count)",
                digest: "sha256:raw-\(index)", size: bytes.count,
                annotations: ["org.opencontainers.image.title": "disk.img.part.\(titleSuffixes[index])"])
            return (layer, bytes)
        }
        let emptyConfig = Layer(
            mediaType: "application/vnd.oci.empty.v1+json", digest: "sha256:empty",
            size: 2, annotations: nil)
        try await assertAssembles(
            parts: parts, expected: payloads.reduce(Data(), +),
            cachingEnabled: cachingEnabled, configDescriptor: emptyConfig)
    }

    private func assertAssembles(
        parts: [(Layer, Data)], expected: Data, cachingEnabled: Bool,
        configDescriptor: Layer? = nil
    ) async throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("lume-legacy-assembly-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }

        let cache = root.appendingPathComponent("cache")
        let registry = ImageContainerRegistry(
            registry: "unused.invalid", organization: "fixture",
            cacheDirectory: cache, cachingEnabled: cachingEnabled)
        let manifestId = "legacy-disk-fixture"
        var layers: [Layer] = []
        for (layer, bytes) in parts {
            let path = registry.getCachedLayerPath(manifestId: manifestId, digest: layer.digest)
            try FileManager.default.createDirectory(
                at: path.deletingLastPathComponent(), withIntermediateDirectories: true)
            try bytes.write(to: path)
            layers.append(layer)
        }
        let config = try JSONSerialization.data(withJSONObject: [
            "annotations": ["com.trycua.lume.disk.uncompressed_size": String(expected.count)]
        ])
        let configLayer = Layer(
            mediaType: "application/vnd.oci.image.config.v1+json", digest: "sha256:config",
            size: config.count, annotations: nil)
        try config.write(to: registry.getCachedLayerPath(
            manifestId: manifestId, digest: configLayer.digest))
        layers.append(configLayer)
        let manifest = Manifest(
            layers: layers, config: configDescriptor ?? configLayer,
            mediaType: "application/vnd.oci.image.manifest.v1+json",
            schemaVersion: 2, annotations: nil)
        #expect(OCIMediaType.detect(manifest) == .legacy)

        let cold = root.appendingPathComponent("cold")
        try FileManager.default.createDirectory(at: cold, withIntermediateDirectories: true)
        try await registry.copyFromCache(manifest: manifest, manifestId: manifestId, to: cold)
        try assertDisk(cold.appendingPathComponent("disk.img"), equals: expected)

        let assembled = registry.getCachedLayerPath(manifestId: manifestId, digest: "unused")
            .deletingLastPathComponent().appendingPathComponent("disk.img.reassembled")
        #expect(FileManager.default.fileExists(atPath: assembled.path) == cachingEnabled)
        if cachingEnabled {
            try assertDisk(assembled, equals: expected)
            // The warm path must serve the assembled artifact, not re-read the parts.
            for (layer, _) in parts {
                #expect(!FileManager.default.fileExists(atPath: registry.getCachedLayerPath(
                    manifestId: manifestId, digest: layer.digest).path))
            }
            let warm = root.appendingPathComponent("warm")
            try FileManager.default.createDirectory(at: warm, withIntermediateDirectories: true)
            try await registry.copyFromCache(manifest: manifest, manifestId: manifestId, to: warm)
            try assertDisk(warm.appendingPathComponent("disk.img"), equals: expected)
        }
    }

    private func assertDisk(_ url: URL, equals expected: Data) throws {
        let actual = try Data(contentsOf: url)
        #expect(actual.count == expected.count)
        #expect(actual.prefix(32) == expected.prefix(32))
        #expect(actual.suffix(32) == expected.suffix(32))
        #expect(actual.elementsEqual(expected), "assembled disk must preserve every source byte")
    }
}
