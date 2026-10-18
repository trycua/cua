import Darwin
import Foundation

/// Recognizes a `lume serve` process, which can host many VMs at once.
///
/// `lume stop` signals the process holding a VM's run lock. When that process
/// is a `lume serve` (the API server Cua and other clients start VMs through),
/// a signal would end the server and every VM it hosts, so `stop` asks the
/// server to stop the one VM instead.
enum LumeServeProcess {
    static let defaultPort: UInt16 = 7777

    /// The port `pid` serves the Lume API on, or nil when `pid` is not a
    /// `lume serve` process (a detached `lume run`, say) or cannot be read.
    static func apiPort(ofProcess pid: pid_t) -> UInt16? {
        guard let arguments = arguments(ofProcess: pid) else { return nil }
        return apiPort(fromArguments: arguments)
    }

    /// The API port in a `lume serve` command line (`argv`, program first),
    /// or nil when the command is not `serve`.
    static func apiPort(fromArguments arguments: [String]) -> UInt16? {
        guard arguments.count >= 2, arguments[1] == "serve" else { return nil }
        var port = defaultPort
        var index = 2
        while index < arguments.count {
            let argument = arguments[index]
            if argument == "--mcp" { return nil }  // stdio transport: no HTTP API
            if argument == "--port", index + 1 < arguments.count {
                guard let value = UInt16(arguments[index + 1]) else { return nil }
                port = value
                index += 2
                continue
            }
            if argument.hasPrefix("--port=") {
                guard let value = UInt16(argument.dropFirst("--port=".count)) else { return nil }
                port = value
            }
            index += 1
        }
        return port
    }

    /// `pid`'s command line from `KERN_PROCARGS2`.
    static func arguments(ofProcess pid: pid_t) -> [String]? {
        var mib: [Int32] = [CTL_KERN, KERN_PROCARGS2, pid]
        var size = 0
        guard sysctl(&mib, 3, nil, &size, nil, 0) == 0, size > MemoryLayout<Int32>.size else {
            return nil
        }
        var buffer = [UInt8](repeating: 0, count: size)
        guard sysctl(&mib, 3, &buffer, &size, nil, 0) == 0 else { return nil }
        return parseProcArgs(Array(buffer.prefix(size)))
    }

    /// Splits a `KERN_PROCARGS2` buffer: `argc`, the executable path, padding
    /// NULs, then `argc` NUL-terminated arguments.
    static func parseProcArgs(_ buffer: [UInt8]) -> [String]? {
        let countSize = MemoryLayout<Int32>.size
        guard buffer.count > countSize else { return nil }
        let argc = buffer.prefix(countSize).withUnsafeBytes { Int($0.loadUnaligned(as: Int32.self)) }
        guard argc > 0 else { return nil }
        var index = countSize
        // Skip the executable path and the NULs after it.
        while index < buffer.count, buffer[index] != 0 { index += 1 }
        while index < buffer.count, buffer[index] == 0 { index += 1 }
        var arguments: [String] = []
        while arguments.count < argc, index < buffer.count {
            let start = index
            while index < buffer.count, buffer[index] != 0 { index += 1 }
            arguments.append(String(decoding: buffer[start..<index], as: UTF8.self))
            index += 1
        }
        return arguments.count == argc ? arguments : nil
    }
}
