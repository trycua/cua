import Foundation
import Testing

@testable import lume

@Test("SSHResult preserves raw output bytes alongside text view")
func testSSHResultPreservesRawOutputBytes() {
    let bytes = Data([0x00, 0xff, 0x41, 0x80, 0x42])
    let result = SSHResult(exitCode: 0, outputData: bytes)

    #expect(result.outputData == bytes)
    #expect(result.output.contains("A"))
    #expect(result.output.contains("B"))
}

