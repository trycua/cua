import Foundation
import Testing

@testable import lume

@Suite struct KCPasswordEncodingTests {
    private let key: [UInt8] = [0x7d, 0x89, 0x52, 0x23, 0xd2, 0xbc, 0xdd, 0xea, 0xa3, 0xb9, 0x1f]

    @Test(arguments: [0, 1, 11, 12, 13, 23, 24, 25])
    func asciiBlockBoundaries(length: Int) {
        verify(String(repeating: "a", count: length))
    }

    @Test(arguments: ["café", String(repeating: "é", count: 6),
                      String(repeating: "U0001f642", count: 3),
                      String(repeating: "e\u{301}", count: 4),
                      String(repeating: "a", count: 11) + "é"])
    func utf8ByteBoundaries(password: String) {
        verify(password)
    }

    private func verify(_ password: String) {
        let plaintext = Array(password.utf8)
        let encoded = MacOSOfflineSetupPatcher.kcpasswordData(for: password)
        #expect(encoded.count == 12 * (plaintext.count / 12 + 1))
        let decoded = encoded.enumerated().map { $0.element ^ key[$0.offset % key.count] }
        #expect(Array(decoded.prefix(plaintext.count)) == plaintext)
        let tail = decoded.dropFirst(plaintext.count)
        #expect(!tail.isEmpty)
        #expect(tail.allSatisfy { $0 == 0 })
    }
}
