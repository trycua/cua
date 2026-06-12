import Foundation
import Testing

@testable import lume

// Tests for the streaming file checksum helper backing layer verification (issue #296).
struct SHA256FileTests {
    private func makeTempFile(_ data: Data) throws -> URL {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("lume-sha256-test-\(UUID().uuidString)")
        try data.write(to: url)
        return url
    }

    @Test func emptyFileMatchesKnownDigest() throws {
        let url = try makeTempFile(Data())
        defer { try? FileManager.default.removeItem(at: url) }

        // Well-known SHA256 of the empty input.
        #expect(
            try sha256OfFile(at: url)
                == "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
    }

    @Test func smallContentMatchesKnownDigest() throws {
        let url = try makeTempFile(Data("abc".utf8))
        defer { try? FileManager.default.removeItem(at: url) }

        // Well-known SHA256 of "abc".
        #expect(
            try sha256OfFile(at: url)
                == "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
    }

    @Test func streamingMatchesInMemoryHashAcrossChunkBoundary() throws {
        // Larger than the 4 MiB streaming chunk so we exercise multiple
        // CC_SHA256_Update calls and confirm the streamed result equals the
        // one-shot Data hash.
        var bytes = Data(count: 5 * 1024 * 1024 + 17)
        for i in stride(from: 0, to: bytes.count, by: 7) {
            bytes[i] = UInt8(i % 251)
        }
        let url = try makeTempFile(bytes)
        defer { try? FileManager.default.removeItem(at: url) }

        #expect(try sha256OfFile(at: url) == bytes.sha256String())
    }

    @Test func differentContentProducesDifferentDigest() throws {
        let a = try makeTempFile(Data("hello".utf8))
        let b = try makeTempFile(Data("world".utf8))
        defer {
            try? FileManager.default.removeItem(at: a)
            try? FileManager.default.removeItem(at: b)
        }

        #expect(try sha256OfFile(at: a) != sha256OfFile(at: b))
    }
}
