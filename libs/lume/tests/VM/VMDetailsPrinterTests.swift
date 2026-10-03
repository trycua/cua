import Foundation
import Testing

@testable import lume

struct VMDetailsPrinterTests {

    @Test(arguments: [80, 120, 274])
    func printStatus_whenJSON(terminalWidth: Int) throws {
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
        try VMDetailsPrinter.printStatus(
            vms, format: .json, terminalWidth: terminalWidth, print: { printedStatus = $0 })

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

    @Test(arguments: [274, 280])
    func printStatus_whenTableFits(terminalWidth: Int) throws {
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
        try VMDetailsPrinter.printStatus(
            vms, format: .text, terminalWidth: terminalWidth, print: { printedLines.append($0) })

        // Then
        #expect(printedLines.count == 2)
        #expect(printedLines.allSatisfy { $0.count == 274 })

        let headerParts = printedLines[0].split(whereSeparator: \.isWhitespace)
        #expect(
            headerParts == [
                "name", "os", "cpu", "memory", "disk", "display", "status", "network", "storage",
                "shared_dirs", "ip", "ssh", "vnc",
            ])

        #expect(
            printedLines[1].split(whereSeparator: \.isWhitespace).map(String.init) == [
                "name", "os", "2", "0.00G", "24.0B/30.0B", "1024x768", "status", "nat",
                "mockLocation",
                "-",
                "0.0.0.0",
                "-",
                "vncUrl",
            ])
    }

    @Test(arguments: [80, 120, 273])
    func printStatus_whenTableDoesNotFit(terminalWidth: Int) throws {
        let vm = VMDetails(
            name: "macos-tahoe",
            os: "macOS",
            cpuCount: 4,
            memorySize: 8 * 1024 * 1024 * 1024,
            diskSize: .init(allocated: 24, total: 30),
            display: "1024x768",
            status: "stopped",
            vncUrl: nil,
            ipAddress: nil,
            locationName: "home")
        let expected = [
            "name:        macos-tahoe",
            "os:          macOS",
            "cpu:         4",
            "memory:      8.00G",
            "disk:        24.0B/30.0B",
            "display:     1024x768",
            "status:      stopped",
            "network:     nat",
            "storage:     home",
            "shared_dirs: -",
            "ip:          -",
            "ssh:         -",
            "vnc:         -",
        ]

        for count in [1, 2] {
            var lines: [String] = []
            try VMDetailsPrinter.printStatus(
                Array(repeating: vm, count: count), format: .text, terminalWidth: terminalWidth,
                print: { lines.append($0) })

            #expect(lines == (count == 1 ? expected : expected + [""] + expected))
            #expect(lines.allSatisfy { $0.count <= terminalWidth })
        }
    }

    @Test func printStatus_whenNarrow_preservesLongValues() throws {
        let name = String(repeating: "macos-", count: 10)
        let path = "/Users/example/" + String(repeating: "shared-project/", count: 6)
        let vncURL = "vnc://:example-password-for-testing@127.0.0.1:59000"
        let vm = VMDetails(
            name: name,
            os: "macOS",
            cpuCount: 4,
            memorySize: 8 * 1024 * 1024 * 1024,
            diskSize: .init(allocated: 24, total: 30),
            display: "1024x768",
            status: "running",
            vncUrl: vncURL,
            ipAddress: "192.168.64.2",
            sshAvailable: true,
            locationName: "home",
            sharedDirectories: [SharedDirectory(hostPath: path, tag: "shared", readOnly: true)],
            networkMode: "bridged:en0")
        var lines: [String] = []

        try VMDetailsPrinter.printStatus(
            [vm], format: .text, terminalWidth: 80, print: { lines.append($0) })

        #expect(lines.contains("name:        \(name)"))
        #expect(lines.contains("shared_dirs: \(path) (ro)"))
        #expect(lines.contains("vnc:         \(vncURL)"))
        #expect(lines.contains("network:     bridged:en0"))
        #expect(lines.contains("ssh:         yes"))
    }
}
