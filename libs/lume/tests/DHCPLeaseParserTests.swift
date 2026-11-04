import Foundation
import Testing

@testable import lume

struct DHCPLeaseParserTests {
    @Test func processOutputReturnsCompletedCommandOutput() {
        let output = DHCPLeaseParser.processOutput(
            executableURL: URL(fileURLWithPath: "/bin/echo"),
            arguments: ["reachable"],
            timeout: 1)

        #expect(output == "reachable\n")
    }

    @Test func processOutputReturnsWhenCommandStalls() {
        let startedAt = Date()
        let output = DHCPLeaseParser.processOutput(
            executableURL: URL(fileURLWithPath: "/bin/sleep"),
            arguments: ["10"],
            timeout: 0.05)

        #expect(output == nil)
        #expect(Date().timeIntervalSince(startedAt) < 1)
    }
}
