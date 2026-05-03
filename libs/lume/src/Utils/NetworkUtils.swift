import Foundation
import Network

enum NetworkUtils {
    /// Runs a process with a timeout, killing it if it exceeds the deadline.
    /// Prevents `lume ls` and other commands from hanging when subprocesses
    /// (lsof, nc, ping) get stuck during concurrent VM state transitions.
    /// - Parameters:
    ///   - process: The configured Process to run
    ///   - timeout: Maximum time to wait in seconds
    /// - Returns: true if the process completed successfully (exit code 0), false otherwise
    private static func runWithTimeout(_ process: Process, timeout: TimeInterval) -> Bool {
        do {
            try process.run()
        } catch {
            return false
        }

        let deadline = Date().addingTimeInterval(timeout)
        // Poll for process completion with 100ms intervals
        while process.isRunning {
            if Date() > deadline {
                process.terminate()
                // Give it a moment to terminate gracefully
                Thread.sleep(forTimeInterval: 0.1)
                if process.isRunning {
                    // Force kill via SIGKILL if terminate (SIGTERM) didn't work
                    kill(process.processIdentifier, SIGKILL)
                }
                return false
            }
            Thread.sleep(forTimeInterval: 0.1)
        }
        return process.terminationStatus == 0
    }

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

        return runWithTimeout(process, timeout: 3)
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

        // Process timeout = nc timeout + 2s buffer for process overhead
        return runWithTimeout(process, timeout: timeout + 2)
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

        // lsof should complete quickly for local port checks;
        // 5s timeout guards against hangs during VM state transitions
        return runWithTimeout(process, timeout: 5)
    }
}
