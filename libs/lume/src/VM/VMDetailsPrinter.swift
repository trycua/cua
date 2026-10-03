import Darwin
import Foundation

/// Prints VM status as a table, or labeled fields in a narrow terminal.
enum VMDetailsPrinter {
    /// Represents a column in the VM status table
    private struct Column: Sendable {
        let header: String
        let width: Int
        let getValue: @Sendable (VMDetails) -> String
    }

    /// Configuration for all columns in the status table
    private static let columns: [Column] = [
        Column(header: "name", width: 40, getValue: { $0.name }),
        Column(header: "os", width: 8, getValue: { $0.os }),
        Column(header: "cpu", width: 8, getValue: { String($0.cpuCount) }),
        Column(
            header: "memory", width: 8,
            getValue: {
                String(format: "%.2fG", Float($0.memorySize) / (1024 * 1024 * 1024))
            }),
        Column(
            header: "disk", width: 16,
            getValue: {
                "\($0.diskSize.formattedAllocated)/\($0.diskSize.formattedTotal)"
            }),
        Column(header: "display", width: 12, getValue: { $0.display }),
        Column(
            header: "status", width: 28,
            getValue: { vm in
                // Show operation type for provisioning status
                if vm.status == "provisioning", let op = vm.provisioningOperation {
                    return "provisioning (\(op))"
                }
                return vm.status
            }),
        Column(header: "network", width: 12, getValue: { $0.networkMode ?? "nat" }),
        Column(header: "storage", width: 16, getValue: { $0.locationName }),
        Column(
            header: "shared_dirs", width: 54,
            getValue: { vm in
                // Only show shared directories if the VM is running
                if vm.status == "running", let dirs = vm.sharedDirectories, !dirs.isEmpty {
                    return dirs.map { "\($0.hostPath) (\($0.readOnly ? "ro" : "rw"))" }.joined(
                        separator: ", ")
                } else {
                    return "-"
                }
            }),
        Column(
            header: "ip", width: 16,
            getValue: {
                $0.ipAddress ?? "-"
            }),
        Column(
            header: "ssh", width: 6,
            getValue: {
                if let ssh = $0.sshAvailable {
                    return ssh ? "yes" : "no"
                }
                return "-"
            }),
        Column(
            header: "vnc", width: 50,
            getValue: {
                $0.vncUrl ?? "-"
            }),
    ]

    /// Prints all fields without truncation when a terminal is too narrow for the table.
    /// JSON and redirected text retain their existing formats.
    /// - Parameter terminalWidth: Available columns; nil detects the width of standard output.
    static func printStatus(
        _ vms: [VMDetails], format: FormatOption, terminalWidth: Int? = nil,
        print: (String) -> Void = { print($0) }
    ) throws {
        if format == .json {
            let jsonEncoder = JSONEncoder()
            jsonEncoder.outputFormatting = .prettyPrinted
            let jsonData = try jsonEncoder.encode(vms)
            let jsonString = String(data: jsonData, encoding: .utf8)!
            print(jsonString)
        } else if let width = try terminalWidth ?? standardOutputWidth(),
            width < columns.reduce(0, { $0 + $1.width })
        {
            let labelWidth = columns.reduce(0) { max($0, $1.header.count) } + 2
            for (index, vm) in vms.enumerated() {
                if index > 0 {
                    print("")
                }
                for column in columns {
                    let label = "\(column.header):".paddedToWidth(labelWidth)
                    print(label + column.getValue(vm))
                }
            }
        } else {
            printHeader(print: print)
            vms.forEach({ vm in
                printVM(vm, print: print)
            })
        }
    }

    /// Non-terminal output has no display width and keeps the stable table layout.
    private static func standardOutputWidth() throws -> Int? {
        guard isatty(STDOUT_FILENO) == 1 else { return nil }

        var size = winsize()
        guard ioctl(STDOUT_FILENO, TIOCGWINSZ, &size) == 0 else {
            throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno))
        }
        return size.ws_col > 0 ? Int(size.ws_col) : nil
    }

    private static func printHeader(print: (String) -> Void = { print($0) }) {
        let paddedHeaders = columns.map { $0.header.paddedToWidth($0.width) }
        print(paddedHeaders.joined())
    }

    private static func printVM(_ vm: VMDetails, print: (String) -> Void = { print($0) }) {
        let paddedColumns = columns.map { column in
            column.getValue(vm).paddedToWidth(column.width)
        }
        print(paddedColumns.joined())
    }
}

extension String {
    /// Pads the string to the specified width with spaces
    /// - Parameter width: Target width for padding
    /// - Returns: Padded string
    fileprivate func paddedToWidth(_ width: Int) -> String {
        padding(toLength: width, withPad: " ", startingAt: 0)
    }
}
