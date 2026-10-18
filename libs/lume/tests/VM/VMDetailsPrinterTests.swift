import Foundation
import Testing

@testable import lume

struct VMDetailsPrinterTests {
    private let vms = ["first", "second"].map { name in
        VMDetails(
            name: name + String(repeating: "-long", count: 10),
            os: "macOS",
            cpuCount: 4,
            memorySize: 8 * 1024 * 1024 * 1024,
            diskSize: .init(allocated: 24, total: 30),
            display: "1024x768",
            status: "running",
            vncUrl: "vnc://:example-password-for-testing-not-a-real-secret@127.0.0.1:59000",
            ipAddress: nil,
            locationName: "home",
            sharedDirectories: [
                SharedDirectory(
                    hostPath: String(repeating: "/shared", count: 10), tag: "shared", readOnly: true
                )
            ])
    }

    @Test(arguments: [true, false])
    func printStatus_whenJSON(isTerminal: Bool) throws {
        var output = ""
        try VMDetailsPrinter.printStatus(
            vms, format: .json, isTerminal: isTerminal, print: { output = $0 })

        let actual = try #require(JSONSerialization.jsonObject(with: Data(output.utf8)) as? NSArray)
        let expected = try JSONSerialization.jsonObject(with: JSONEncoder().encode(vms)) as? NSArray
        #expect(actual == expected)
    }

    @Test func printStatus_whenTerminal() throws {
        var lines: [String] = []
        try VMDetailsPrinter.printStatus(
            vms, format: .text, isTerminal: true, print: { lines.append($0) })

        let expected = vms.map { vm in
            """
            name:        \(vm.name)
            os:          macOS
            cpu:         4
            memory:      8.00G
            disk:        24.0B/30.0B
            display:     1024x768
            status:      running
            network:     nat
            storage:     home
            shared_dirs: \(vm.sharedDirectories![0].hostPath) (ro)
            ip:          -
            ssh:         -
            vnc:         \(vm.vncUrl!)
            """
        }.joined(separator: "\n\n")
        #expect(lines.joined(separator: "\n") == expected)
    }

    @Test func printStatus_whenRedirected() throws {
        var lines: [String] = []
        try VMDetailsPrinter.printStatus(
            vms, format: .text, isTerminal: false, print: { lines.append($0) })

        #expect(lines.count == 3)
        #expect(lines.allSatisfy { $0.count == 274 })
        #expect(
            lines[0].split(whereSeparator: \.isWhitespace) == [
                "name", "os", "cpu", "memory", "disk", "display", "status", "network", "storage",
                "shared_dirs", "ip", "ssh", "vnc",
            ])
        for (line, vm) in zip(lines.dropFirst(), vms) {
            let expected = [
                (vm.name, 40), ("macOS", 8), ("4", 8), ("8.00G", 8), ("24.0B/30.0B", 16),
                ("1024x768", 12), ("running", 28), ("nat", 12), ("home", 16),
                ("\(vm.sharedDirectories![0].hostPath) (ro)", 54), ("-", 16), ("-", 6),
                (vm.vncUrl!, 50),
            ].map { value, width in
                value.padding(toLength: width, withPad: " ", startingAt: 0)
            }.joined()
            #expect(line == expected)
        }
    }
}
