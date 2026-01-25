import Foundation

/// Result of an SSH command execution
public struct SSHResult: Sendable {
    public let exitCode: Int32
    public let output: String

    public init(exitCode: Int32, output: String) {
        self.exitCode = exitCode
        self.output = output
    }
}

/// SSH client that handles password authentication via PTY
/// This eliminates the need for sshpass as an external dependency
public actor SSHClient {
    private let host: String
    private let port: UInt16
    private let user: String
    private let password: String
    private let sshPath: String

    public init(
        host: String,
        port: UInt16 = 22,
        user: String = "lume",
        password: String = "lume"
    ) {
        self.host = host
        self.port = port
        self.user = user
        self.password = password
        self.sshPath = "/usr/bin/ssh"
    }

    /// Execute a command on the remote host
    public func execute(command: String, timeout: TimeInterval = 60) async throws -> SSHResult {
        let args = buildSSHArgs(command: command)
        return try await runWithPTY(args: args, timeout: timeout)
    }

    /// Start an interactive SSH session
    public func interactive() async throws {
        let args = buildSSHArgs(command: nil)
        try await runInteractive(args: args)
    }

    private func buildSSHArgs(command: String?) -> [String] {
        var args = [
            sshPath,
            "-o", "StrictHostKeyChecking=no",
            "-o", "UserKnownHostsFile=/dev/null",
            "-o", "LogLevel=ERROR",
            "-o", "ConnectTimeout=30",
            "-p", String(port),
            "\(user)@\(host)"
        ]
        if let command = command {
            args.append(command)
        }
        return args
    }

    /// Run SSH with PTY for password authentication (non-interactive command)
    private func runWithPTY(args: [String], timeout: TimeInterval) async throws -> SSHResult {
        return try await withCheckedThrowingContinuation { continuation in
            DispatchQueue.global(qos: .userInitiated).async {
                do {
                    let result = try self.executePTYSync(args: args, timeout: timeout, interactive: false)
                    continuation.resume(returning: result)
                } catch {
                    continuation.resume(throwing: error)
                }
            }
        }
    }

    /// Run interactive SSH session with PTY
    private func runInteractive(args: [String]) async throws {
        return try await withCheckedThrowingContinuation { continuation in
            DispatchQueue.global(qos: .userInitiated).async {
                do {
                    _ = try self.executePTYSync(args: args, timeout: 0, interactive: true)
                    continuation.resume()
                } catch {
                    continuation.resume(throwing: error)
                }
            }
        }
    }

    /// Synchronous PTY execution - handles password injection
    private nonisolated func executePTYSync(args: [String], timeout: TimeInterval, interactive: Bool) throws -> SSHResult {
        var masterFD: Int32 = 0
        var winSize = winsize(ws_row: 24, ws_col: 80, ws_xpixel: 0, ws_ypixel: 0)

        // Fork with PTY
        let pid = forkpty(&masterFD, nil, nil, &winSize)

        if pid < 0 {
            throw SSHError.forkFailed
        }

        if pid == 0 {
            // Child process - execute SSH
            let cArgs = args.map { strdup($0) } + [nil]
            execv(args[0], cArgs)
            // If we get here, exec failed
            exit(1)
        }

        // Parent process - handle I/O
        defer {
            close(masterFD)
            var status: Int32 = 0
            waitpid(pid, &status, 0)
        }

        if interactive {
            return try handleInteractiveSession(masterFD: masterFD, pid: pid)
        } else {
            return try handleCommandSession(masterFD: masterFD, pid: pid, timeout: timeout, password: password)
        }
    }

    /// Handle non-interactive command execution with password injection
    private nonisolated func handleCommandSession(masterFD: Int32, pid: pid_t, timeout: TimeInterval, password: String) throws -> SSHResult {
        var output = ""
        var passwordSent = false
        let startTime = Date()
        let buffer = UnsafeMutablePointer<CChar>.allocate(capacity: 4096)
        defer { buffer.deallocate() }

        // Set non-blocking
        let flags = fcntl(masterFD, F_GETFL)
        fcntl(masterFD, F_SETFL, flags | O_NONBLOCK)

        while true {
            // Check timeout
            if timeout > 0 && Date().timeIntervalSince(startTime) > timeout {
                kill(pid, SIGTERM)
                throw SSHError.timeout
            }

            // Check if child exited
            var status: Int32 = 0
            let waitResult = waitpid(pid, &status, WNOHANG)
            if waitResult > 0 {
                // Child exited - read any remaining output
                while true {
                    let bytesRead = read(masterFD, buffer, 4095)
                    if bytesRead <= 0 { break }
                    buffer[bytesRead] = 0
                    output += String(cString: buffer)
                }

                let exitCode = WIFEXITED(status) ? WEXITSTATUS(status) : -1
                return SSHResult(exitCode: exitCode, output: cleanOutput(output))
            }

            // Read available data
            let bytesRead = read(masterFD, buffer, 4095)
            if bytesRead > 0 {
                buffer[bytesRead] = 0
                let chunk = String(cString: buffer)
                output += chunk

                // Check for password prompt
                if !passwordSent && containsPasswordPrompt(output) {
                    // Send password
                    let passwordWithNewline = password + "\n"
                    _ = passwordWithNewline.withCString { ptr in
                        write(masterFD, ptr, strlen(ptr))
                    }
                    passwordSent = true
                }
            } else if bytesRead < 0 && errno != EAGAIN && errno != EWOULDBLOCK {
                break
            }

            // Small sleep to avoid busy loop
            usleep(10000) // 10ms
        }

        // Wait for child
        var status: Int32 = 0
        waitpid(pid, &status, 0)
        let exitCode = WIFEXITED(status) ? WEXITSTATUS(status) : -1
        return SSHResult(exitCode: exitCode, output: cleanOutput(output))
    }

    /// Handle interactive session - pass through stdin/stdout
    private nonisolated func handleInteractiveSession(masterFD: Int32, pid: pid_t) throws -> SSHResult {
        var passwordSent = false
        let buffer = UnsafeMutablePointer<CChar>.allocate(capacity: 4096)
        defer { buffer.deallocate() }

        // Save terminal settings
        var originalTermios = termios()
        tcgetattr(STDIN_FILENO, &originalTermios)

        // Set raw mode for stdin
        var rawTermios = originalTermios
        cfmakeraw(&rawTermios)
        tcsetattr(STDIN_FILENO, TCSANOW, &rawTermios)

        defer {
            // Restore terminal settings
            tcsetattr(STDIN_FILENO, TCSANOW, &originalTermios)
        }

        // Set up file descriptors for select
        let stdinFD = STDIN_FILENO
        var accumulatedOutput = ""
        let password = self.password

        while true {
            var readSet = fd_set()
            withUnsafeMutablePointer(to: &readSet) { ptr in
                __darwin_fd_zero(ptr)
                __darwin_fd_set(masterFD, ptr)
                __darwin_fd_set(stdinFD, ptr)
            }

            var timeout = timeval(tv_sec: 1, tv_usec: 0)
            let maxFD = max(masterFD, stdinFD) + 1

            let selectResult = select(maxFD, &readSet, nil, nil, &timeout)
            if selectResult < 0 {
                if errno == EINTR { continue }
                break
            }

            // Check if child exited
            var status: Int32 = 0
            let waitResult = waitpid(pid, &status, WNOHANG)
            if waitResult > 0 {
                let exitCode = WIFEXITED(status) ? WEXITSTATUS(status) : -1
                return SSHResult(exitCode: exitCode, output: "")
            }

            // Check for data from SSH
            if __darwin_fd_isset(masterFD, &readSet) != 0 {
                let bytesRead = read(masterFD, buffer, 4095)
                if bytesRead > 0 {
                    buffer[bytesRead] = 0
                    let chunk = String(cString: buffer)
                    accumulatedOutput += chunk

                    // Auto-inject password if we see prompt
                    if !passwordSent && containsPasswordPrompt(accumulatedOutput) {
                        let passwordWithNewline = password + "\n"
                        _ = passwordWithNewline.withCString { ptr in
                            write(masterFD, ptr, strlen(ptr))
                        }
                        passwordSent = true
                    } else {
                        // Write to stdout
                        write(STDOUT_FILENO, buffer, bytesRead)
                    }
                } else if bytesRead == 0 {
                    break
                }
            }

            // Check for data from stdin
            if __darwin_fd_isset(stdinFD, &readSet) != 0 {
                let bytesRead = read(stdinFD, buffer, 4095)
                if bytesRead > 0 {
                    write(masterFD, buffer, bytesRead)
                } else if bytesRead == 0 {
                    break
                }
            }
        }

        var status: Int32 = 0
        waitpid(pid, &status, 0)
        let exitCode = WIFEXITED(status) ? WEXITSTATUS(status) : -1
        return SSHResult(exitCode: exitCode, output: "")
    }

    /// Check if output contains a password prompt
    private nonisolated func containsPasswordPrompt(_ output: String) -> Bool {
        let lowercased = output.lowercased()
        return lowercased.contains("password:") ||
               lowercased.contains("password for") ||
               lowercased.hasSuffix("'s password: ")
    }

    /// Clean output by removing password prompts and other noise
    private nonisolated func cleanOutput(_ output: String) -> String {
        var lines = output.components(separatedBy: "\n")

        // Remove lines containing password prompts
        lines = lines.filter { line in
            let lowercased = line.lowercased()
            return !lowercased.contains("password:") &&
                   !lowercased.contains("password for")
        }

        return lines.joined(separator: "\n").trimmingCharacters(in: .whitespacesAndNewlines)
    }
}

// MARK: - Process status macros (Swift equivalents of C macros from sys/wait.h)

/// Check if process terminated normally (equivalent to C WIFEXITED macro)
private func WIFEXITED(_ status: Int32) -> Bool {
    return (status & 0x7f) == 0
}

/// Extract exit status from terminated process (equivalent to C WEXITSTATUS macro)
private func WEXITSTATUS(_ status: Int32) -> Int32 {
    return (status >> 8) & 0xff
}

// MARK: - fd_set helpers for Darwin

private func __darwin_fd_zero(_ set: UnsafeMutablePointer<fd_set>) {
    set.pointee.fds_bits = (0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0)
}

private func __darwin_fd_set(_ fd: Int32, _ set: UnsafeMutablePointer<fd_set>) {
    let intOffset = Int(fd) / 32
    let bitOffset = Int(fd) % 32
    withUnsafeMutablePointer(to: &set.pointee.fds_bits) { ptr in
        let rawPtr = UnsafeMutableRawPointer(ptr).assumingMemoryBound(to: Int32.self)
        rawPtr[intOffset] |= Int32(1 << bitOffset)
    }
}

private func __darwin_fd_isset(_ fd: Int32, _ set: UnsafePointer<fd_set>) -> Int32 {
    let intOffset = Int(fd) / 32
    let bitOffset = Int(fd) % 32
    return withUnsafePointer(to: set.pointee.fds_bits) { ptr in
        let rawPtr = UnsafeRawPointer(ptr).assumingMemoryBound(to: Int32.self)
        return (rawPtr[intOffset] & Int32(1 << bitOffset)) != 0 ? 1 : 0
    }
}
