import Darwin
import Foundation
import MCP

/// Blocking client that sends a single request to a running daemon and
/// returns the response. Used by `cua-driver call` / `list-tools` /
/// `describe` / `status` / `stop`.
///
/// Intentionally not an actor — it's one-shot per CLI invocation. One
/// connect, one write, one read, one close.
public enum DaemonClient {
    /// Connect to the socket, write `request`, read one JSON line back,
    /// decode it. Returns `nil` if the socket can't be reached, which the
    /// CLI treats as "no daemon running → fall back to in-process".
    public static func sendRequest(
        _ request: DaemonRequest,
        socketPath: String,
        connectTimeout: TimeInterval = 0.25,
        readTimeout: TimeInterval = 120.0
    ) -> DaemonCallResult {
        guard let fd = connect(socketPath: socketPath, timeout: connectTimeout) else {
            return .noDaemon
        }
        defer { close(fd) }

        do {
            let encoder = JSONEncoder()
            encoder.outputFormatting = [.withoutEscapingSlashes]
            var payload = try encoder.encode(request)
            payload.append(0x0A)
            guard writeAll(fd: fd, data: payload) else {
                return .error("failed to write request to daemon socket")
            }
            // Half-close write side would be nice but the daemon expects a
            // persistent connection for potential batched calls. We just
            // wait for the response line.
            guard let line = readLine(fd: fd, timeout: readTimeout) else {
                return .error("daemon closed connection before responding")
            }
            let response = try JSONDecoder().decode(DaemonResponse.self, from: line)
            return .ok(response)
        } catch {
            return .error("daemon protocol error: \(error)")
        }
    }

    /// Probe for a running daemon. Returns `true` only when a TCP-style
    /// connect succeeds on the UDS — a stale socket file that can't be
    /// connected to (e.g. crashed prior daemon) returns `false`.
    public static func isDaemonListening(socketPath: String) -> Bool {
        guard let fd = connect(socketPath: socketPath, timeout: 0.25) else {
            return false
        }
        close(fd)
        return true
    }

    // MARK: - socket plumbing

    private static func connect(socketPath: String, timeout: TimeInterval) -> Int32? {
        // Fast-path: if the path doesn't even exist, don't bother calling
        // connect() — spares us the EACCES/ENOENT noise in error paths.
        var stBuf = stat()
        if stat(socketPath, &stBuf) != 0 { return nil }

        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { return nil }

        // Disable SIGPIPE on write to a broken peer — prefer EPIPE over
        // a process-terminating signal if the daemon dies mid-request.
        var noSigPipe: Int32 = 1
        _ = setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &noSigPipe, socklen_t(MemoryLayout<Int32>.size))

        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        let pathBytes = Array(socketPath.utf8)
        let maxLen = MemoryLayout.size(ofValue: addr.sun_path) - 1
        guard pathBytes.count <= maxLen else {
            close(fd)
            return nil
        }
        withUnsafeMutableBytes(of: &addr.sun_path) { raw in
            let buffer = raw.bindMemory(to: UInt8.self)
            for (index, byte) in pathBytes.enumerated() { buffer[index] = byte }
            buffer[pathBytes.count] = 0
        }

        // Simple blocking connect — we only wait ~250ms so a dead socket
        // doesn't hang the CLI.
        let addrLen = socklen_t(MemoryLayout<sockaddr_un>.size)
        // Bound send/recv so a wedged daemon can't stall write()/read()
        // indefinitely. Note: this does not cover connect() itself.
        var tv = timeval(
            tv_sec: Int(timeout),
            tv_usec: __darwin_suseconds_t((timeout - Double(Int(timeout))) * 1_000_000)
        )
        _ = setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &tv, socklen_t(MemoryLayout<timeval>.size))

        let result = withUnsafePointer(to: &addr) { addrPtr in
            addrPtr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockPtr in
                Darwin.connect(fd, sockPtr, addrLen)
            }
        }
        if result != 0 {
            close(fd)
            return nil
        }
        return fd
    }

    private static func writeAll(fd: Int32, data: Data) -> Bool {
        return data.withUnsafeBytes { raw -> Bool in
            guard let base = raw.baseAddress else { return false }
            var remaining = raw.count
            var offset = 0
            while remaining > 0 {
                let n = write(fd, base.advanced(by: offset), remaining)
                if n <= 0 { return false }
                offset += n
                remaining -= n
            }
            return true
        }
    }

    private static func readLine(fd: Int32, timeout: TimeInterval) -> Data? {
        // Apply read timeout to the fd so we don't block forever if the
        // daemon process hangs.
        var tv = timeval(
            tv_sec: Int(timeout),
            tv_usec: __darwin_suseconds_t((timeout - Double(Int(timeout))) * 1_000_000)
        )
        _ = setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, socklen_t(MemoryLayout<timeval>.size))

        var buffer = Data()
        var chunk = [UInt8](repeating: 0, count: 4096)
        while true {
            let n = chunk.withUnsafeMutableBufferPointer { buf -> Int in
                read(fd, buf.baseAddress, buf.count)
            }
            if n <= 0 { return nil }
            buffer.append(contentsOf: chunk[0..<n])
            if let newlineIndex = buffer.firstIndex(of: 0x0A) {
                return buffer.prefix(upTo: newlineIndex)
            }
        }
    }
}

public enum DaemonCallResult {
    case ok(DaemonResponse)
    case noDaemon
    case error(String)
}

/// Read the pid from the daemon's pid file, if present and parseable.
/// Returns nil otherwise — `cua-driver status` treats that as "unknown pid".
public func readDaemonPid(from pidFilePath: String) -> Int32? {
    guard let contents = try? String(contentsOfFile: pidFilePath, encoding: .utf8) else {
        return nil
    }
    return Int32(contents.trimmingCharacters(in: .whitespacesAndNewlines))
}
