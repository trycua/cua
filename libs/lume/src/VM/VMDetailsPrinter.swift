import Foundation

/// Prints VM status information in a formatted table
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
        Column(header: "storage", width: 16, getValue: { $0.locationName }),
        Column(
            header: "shared_dirs", width: 54,
            getValue: { vm in
                // Only show shared directories if the VM is running
                if vm.status == "running", let dirs = vm.sharedDirectories, !dirs.isEmpty {
                    return dirs.map { "\($0.hostPath) (\($0.readOnly ? "ro" : "rw"))" }.joined(separator: ", ")
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

    /// Prints the status of all VMs in a formatted table
    /// - Parameter vms: Array of VM status objects to display
    static func printStatus(
        _ vms: [VMDetails], format: FormatOption, print: (String) -> Void = { print($0) }
    ) throws {
        if format == .json {
            let jsonEncoder = JSONEncoder()
            jsonEncoder.outputFormatting = .prettyPrinted
            let jsonData = try jsonEncoder.encode(vms)
            let jsonString = String(data: jsonData, encoding: .utf8)!
            print(jsonString)
        } else {
            printHeader(print: print)
            vms.forEach({ vm in 
                printVM(vm, print: print)
            })
        }
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
