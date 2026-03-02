import Foundation

/// SSH client that delegates to the system's /usr/bin/ssh binary.
/// Used as a fallback when NIO SSH cannot establish a direct TCP connection
/// (e.g., in sandboxed environments where only system-signed binaries can
/// access certain network interfaces like vmnet).
///
/// Uses SSH_ASKPASS to provide password authentication non-interactively.
public final class SystemSSHClient: Sendable {
    private let host: String
    private let port: Int
    private let user: String
    private let password: String

    public init(
        host: String,
        port: UInt16 = 22,
        user: String = "lume",
        password: String = "lume"
    ) {
        self.host = host
        self.port = Int(port)
        self.user = user
        self.password = password
    }

    /// Execute a command on the remote host using system ssh
    public func execute(command: String, timeout: TimeInterval = 60) throws -> SSHResult {
        let askpassPath = try createAskpassScript()
        defer { try? FileManager.default.removeItem(atPath: askpassPath) }

        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/ssh")
        process.arguments = sshArguments(extraArgs: ["\(user)@\(host)", command])

        var environment = ProcessInfo.processInfo.environment
        environment["SSH_ASKPASS"] = askpassPath
        environment["SSH_ASKPASS_REQUIRE"] = "force"
        environment["DISPLAY"] = ":0"
        process.environment = environment

        let stdoutPipe = Pipe()
        let stderrPipe = Pipe()
        process.standardOutput = stdoutPipe
        process.standardError = stderrPipe
        // Detach from controlling terminal so SSH_ASKPASS is used
        process.standardInput = FileHandle.nullDevice

        try process.run()

        // Set up timeout
        if timeout > 0 {
            DispatchQueue.global().asyncAfter(deadline: .now() + timeout) {
                if process.isRunning {
                    process.terminate()
                }
            }
        }

        process.waitUntilExit()

        let stdoutData = stdoutPipe.fileHandleForReading.readDataToEndOfFile()
        let stderrData = stderrPipe.fileHandleForReading.readDataToEndOfFile()
        let output = String(data: stdoutData, encoding: .utf8) ?? ""
        let errorOutput = String(data: stderrData, encoding: .utf8) ?? ""

        // Filter out SSH warnings from stderr (known_hosts, etc.)
        let filteredError = errorOutput.components(separatedBy: "\n")
            .filter { line in
                !line.contains("Warning: Permanently added") &&
                !line.contains("known_hosts") &&
                !line.trimmingCharacters(in: .whitespaces).isEmpty
            }
            .joined(separator: "\n")

        let combinedOutput = filteredError.isEmpty ? output : output + filteredError

        return SSHResult(
            exitCode: process.terminationStatus,
            output: combinedOutput
        )
    }

    /// Start an interactive SSH session using system ssh
    public func interactive() throws {
        let askpassPath = try createAskpassScript()
        defer { try? FileManager.default.removeItem(atPath: askpassPath) }

        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/ssh")
        process.arguments = sshArguments(extraArgs: ["-t", "\(user)@\(host)"])

        var environment = ProcessInfo.processInfo.environment
        environment["SSH_ASKPASS"] = askpassPath
        environment["SSH_ASKPASS_REQUIRE"] = "force"
        environment["DISPLAY"] = ":0"
        process.environment = environment

        // For interactive mode, pass through stdin/stdout/stderr
        process.standardInput = FileHandle.standardInput
        process.standardOutput = FileHandle.standardOutput
        process.standardError = FileHandle.standardError

        try process.run()
        process.waitUntilExit()

        if process.terminationStatus != 0 {
            throw SSHError.connectionFailed(
                "System SSH exited with code \(process.terminationStatus)"
            )
        }
    }

    // MARK: - Private

    private func sshArguments(extraArgs: [String]) -> [String] {
        var args = [
            "-o", "StrictHostKeyChecking=no",
            "-o", "UserKnownHostsFile=/dev/null",
            "-o", "LogLevel=ERROR",
            "-o", "ConnectTimeout=10",
        ]

        if port != 22 {
            args += ["-p", "\(port)"]
        }

        args += extraArgs
        return args
    }

    /// Creates a temporary script that outputs the password for SSH_ASKPASS
    private func createAskpassScript() throws -> String {
        let tempDir = FileManager.default.temporaryDirectory
        let scriptPath = tempDir.appendingPathComponent("lume-askpass-\(UUID().uuidString).sh").path

        let scriptContent = """
            #!/bin/sh
            echo '\(password.replacingOccurrences(of: "'", with: "'\\''"))'
            """

        guard FileManager.default.createFile(
            atPath: scriptPath,
            contents: scriptContent.data(using: .utf8),
            attributes: [.posixPermissions: 0o700]
        ) else {
            throw SSHError.connectionFailed("Failed to create askpass script")
        }

        return scriptPath
    }
}
