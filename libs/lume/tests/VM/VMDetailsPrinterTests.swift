import Foundation
import Testing

@testable import lume

struct VMDetailsPrinterTests {

    @Test func printStatus_whenJSON() throws {
        // Given
        let vms: [VMDetails] = [
            VMDetails(
                name: "name",
                os: "os",
                cpuCount: 2,
                memorySize: 1024,
                diskSize: .init(allocated: 24, total: 30),
                display: "1024x768",
                status: "status",
                vncUrl: "vncUrl",
                ipAddress: "0.0.0.0",
                locationName: "mockLocation")
        ]
        let jsonEncoder = JSONEncoder()
        jsonEncoder.outputFormatting = .prettyPrinted
        let expectedOutput = try String(data: jsonEncoder.encode(vms), encoding: .utf8)!

        // When
        var printedStatus: String?
        try VMDetailsPrinter.printStatus(vms, format: .json, print: { printedStatus = $0 })

        // Then
        // Decode both JSONs and compare the actual data structures
        let jsonDecoder = JSONDecoder()
        let printedVMs = try jsonDecoder.decode(
            [VMDetails].self, from: printedStatus!.data(using: .utf8)!)
        let expectedVMs = try jsonDecoder.decode(
            [VMDetails].self, from: expectedOutput.data(using: .utf8)!)

        #expect(printedVMs.count == expectedVMs.count)
        for (printed, expected) in zip(printedVMs, expectedVMs) {
            #expect(printed.name == expected.name)
            #expect(printed.os == expected.os)
            #expect(printed.cpuCount == expected.cpuCount)
            #expect(printed.memorySize == expected.memorySize)
            #expect(printed.diskSize.allocated == expected.diskSize.allocated)
            #expect(printed.diskSize.total == expected.diskSize.total)
            #expect(printed.status == expected.status)
            #expect(printed.vncUrl == expected.vncUrl)
            #expect(printed.ipAddress == expected.ipAddress)
        }
    }

    @Test func printStatus_whenNotJSON() throws {
        // Given
        let vms: [VMDetails] = [
            VMDetails(
                name: "name",
                os: "os",
                cpuCount: 2,
                memorySize: 1024,
                diskSize: .init(allocated: 24, total: 30),
                display: "1024x768",
                status: "status",
                vncUrl: "vncUrl",
                ipAddress: "0.0.0.0",
                locationName: "mockLocation")
        ]

        // When
        var printedLines: [String] = []
        try VMDetailsPrinter.printStatus(vms, format: .text, print: { printedLines.append($0) })

        // Then
        #expect(printedLines.count == 2)

        let headerParts = printedLines[0].split(whereSeparator: \.isWhitespace)
        #expect(
            headerParts == [
                "name", "os", "cpu", "memory", "disk", "display", "status", "storage", "ip", "vnc",
            ])

        #expect(
            printedLines[1].split(whereSeparator: \.isWhitespace).map(String.init) == [
                "name", "os", "2", "0.00G", "24.0B/30.0B", "1024x768", "status", "mockLocation",
                "0.0.0.0",
                "vncUrl",
            ])
    }
}
