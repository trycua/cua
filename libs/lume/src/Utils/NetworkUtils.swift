import Foundation
import Network

enum NetworkUtils {
    /// Checks if an IP address is reachable by sending a ping
    /// - Parameter ipAddress: The IP address to check
    /// - Returns: true if the IP is reachable, false otherwise
    static func isReachable(ipAddress: String) -> Bool {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/sbin/ping")
        process.arguments = ["-c", "1", "-t", "1", ipAddress]

        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = pipe

        do {
            try process.run()
            process.waitUntilExit()
            return process.terminationStatus == 0
        } catch {
            return false
        }
    }

    /// Checks if a TCP port is open on an IP address
    /// - Parameters:
    ///   - ipAddress: The IP address to check
    ///   - port: The port number to check
    ///   - timeout: Timeout in seconds (default: 2)
    /// - Returns: true if the port is open, false otherwise
    static func isPortOpen(ipAddress: String, port: UInt16, timeout: TimeInterval = 2) -> Bool {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/nc")
        // Use -w (timeout) instead of -G (connection timeout) as -G doesn't work
        // reliably with VM networking on macOS
        process.arguments = ["-z", "-w", "\(Int(timeout))", ipAddress, "\(port)"]

        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = pipe

        do {
            try process.run()
            process.waitUntilExit()
            return process.terminationStatus == 0
        } catch {
            return false
        }
    }

    /// Checks if SSH (port 22) is available on an IP address
    /// - Parameter ipAddress: The IP address to check
    /// - Returns: true if SSH port is open, false otherwise
    static func isSSHAvailable(ipAddress: String) -> Bool {
        return isPortOpen(ipAddress: ipAddress, port: 22, timeout: 2)
    }

    /// Checks if a local port has a process listening on it
    /// Uses lsof which is fast for local port checks
    /// - Parameter port: The port number to check
    /// - Returns: true if a process is listening on the port, false otherwise
    static func isLocalPortInUse(port: UInt16) -> Bool {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/sbin/lsof")
        process.arguments = ["-i", ":\(port)", "-sTCP:LISTEN"]

        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = pipe

        do {
            try process.run()
            process.waitUntilExit()
            // lsof returns 0 if it finds a matching process
            return process.terminationStatus == 0
        } catch {
            return false
        }
    }
}
