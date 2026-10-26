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

    /// Checks if a TCP port is open on an IP address: a non-blocking
    /// connect, waited on for at most `timeout`. No process is spawned, so a
    /// guest that does not answer yet costs `timeout`, not seconds of `nc`
    /// (callers run on the main actor, which `lume serve` shares with every
    /// request and with VM start and restore).
    /// - Parameters:
    ///   - ipAddress: The IPv4 address to check
    ///   - port: The port number to check
    ///   - timeout: Timeout in seconds (default: 2)
    /// - Returns: true if the port accepted the connection, false otherwise
    static func isPortOpen(ipAddress: String, port: UInt16, timeout: TimeInterval = 2) -> Bool {
        var addr = sockaddr_in()
        addr.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
        addr.sin_family = sa_family_t(AF_INET)
        addr.sin_port = port.bigEndian
        guard inet_pton(AF_INET, ipAddress, &addr.sin_addr) == 1 else { return false }
        let fd = socket(AF_INET, SOCK_STREAM, 0)
        guard fd >= 0 else { return false }
        defer { close(fd) }
        _ = fcntl(fd, F_SETFL, fcntl(fd, F_GETFL) | O_NONBLOCK)
        let rc = withUnsafePointer(to: &addr) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                connect(fd, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
            }
        }
        if rc == 0 { return true }
        guard errno == EINPROGRESS else { return false }
        var pfd = pollfd(fd: fd, events: Int16(POLLOUT), revents: 0)
        guard poll(&pfd, 1, Int32(max(timeout, 0) * 1000)) == 1 else { return false }
        var err: Int32 = 0
        var len = socklen_t(MemoryLayout<Int32>.size)
        guard getsockopt(fd, SOL_SOCKET, SO_ERROR, &err, &len) == 0 else { return false }
        return err == 0
    }

    /// Checks if SSH (port 22) is available on an IP address
    /// - Parameter ipAddress: The IP address to check
    /// - Returns: true if SSH port is open, false otherwise
    static func isSSHAvailable(ipAddress: String) -> Bool {
        return isPortOpen(ipAddress: ipAddress, port: 22, timeout: 0.3)
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
