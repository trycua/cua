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
        process.arguments = ["-z", "-G", "\(Int(timeout))", ipAddress, "\(port)"]

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
} 