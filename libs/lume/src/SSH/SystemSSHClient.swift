import Foundation

enum RemoteDesktopCopyError: Error, LocalizedError {
    case noFilesSelected
    case unavailableItem
    case duplicateNames
    case destinationExists
    case preparationFailed(String?)
    case transferFailed(String?)
    case commitFailed(String?)
    case timedOut

    var errorDescription: String? {
        switch self {
        case .noFilesSelected:
            return "No files were selected"
        case .unavailableItem:
            return "A dropped item is no longer available"
        case .duplicateNames:
            return "Dropped items must have unique names"
        case .destinationExists:
            return "An item with the same name already exists on the VM Desktop"
        case .preparationFailed(let detail):
            return detail.map { "Could not prepare the VM Desktop — \($0)" }
                ?? "Could not prepare the VM Desktop"
        case .transferFailed(let detail):
            return detail.map { "Could not copy the dropped items — \($0)" }
                ?? "Could not copy the dropped items"
        case .commitFailed(let detail):
            return detail.map { "Could not place the dropped items on the VM Desktop — \($0)" }
                ?? "Could not place the dropped items on the VM Desktop"
        case .timedOut:
            return "File copy timed out"
        }
    }
}

final class CopyProcessController: @unchecked Sendable {
    private enum StopReason {
        case cancelled
        case timedOut
    }

    private let lock = NSLock()
    private var process: Process?
    private var stopReason: StopReason?
    private var timeoutWorkItem: DispatchWorkItem?

    func register(_ process: Process) throws {
        lock.lock()
        defer { lock.unlock() }
        if stopReason == .cancelled {
            throw CancellationError()
        }
        if stopReason == .timedOut {
            throw RemoteDesktopCopyError.timedOut
        }
        self.process = process
    }

    func processDidStart(_ process: Process) {
        lock.lock()
        let shouldTerminate = self.process === process && stopReason != nil
        lock.unlock()
        if shouldTerminate, process.isRunning {
            process.terminate()
        }
    }

    func cancel() {
        requestStop(.cancelled, process: nil)
    }

    func timeOut(_ process: Process) {
        requestStop(.timedOut, process: process)
    }

    func scheduleTimeout(for process: Process, deadline: Date) {
        let remaining = deadline.timeIntervalSinceNow
        guard remaining > 0 else {
            timeOut(process)
            return
        }
        let workItem = DispatchWorkItem { [weak self, weak process] in
            guard let self, let process else { return }
            self.timeOut(process)
        }
        lock.lock()
        timeoutWorkItem?.cancel()
        timeoutWorkItem = workItem
        lock.unlock()
        DispatchQueue.global().asyncAfter(deadline: .now() + remaining, execute: workItem)
    }

    func finish(_ process: Process) throws {
        lock.lock()
        let reason = self.process === process ? stopReason : nil
        if self.process === process {
            self.process = nil
        }
        timeoutWorkItem?.cancel()
        timeoutWorkItem = nil
        lock.unlock()

        switch reason {
        case .cancelled:
            throw CancellationError()
        case .timedOut:
            throw RemoteDesktopCopyError.timedOut
        case nil:
            return
        }
    }

    private func requestStop(_ reason: StopReason, process expectedProcess: Process?) {
        lock.lock()
        guard expectedProcess == nil || process === expectedProcess else {
            lock.unlock()
            return
        }
        if expectedProcess != nil, process?.isRunning != true {
            lock.unlock()
            return
        }
        if stopReason == nil {
            stopReason = reason
        }
        let activeProcess = process
        lock.unlock()

        if let activeProcess, activeProcess.isRunning {
            activeProcess.terminate()
            let processIdentifier = activeProcess.processIdentifier
            DispatchQueue.global().asyncAfter(deadline: .now() + 2) {
                if activeProcess.isRunning {
                    kill(processIdentifier, SIGKILL)
                }
            }
        }
    }
}

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

    /// Copy host files or directories to the guest user's Desktop using system scp.
    public func copyToRemoteDesktop(
        _ urls: [URL],
        timeout: TimeInterval = 600
    ) async throws {
        let controller = CopyProcessController()
        try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                DispatchQueue.global(qos: .userInitiated).async {
                    do {
                        try self.copyToRemoteDesktopBlocking(
                            urls,
                            timeout: timeout,
                            controller: controller
                        )
                        continuation.resume()
                    } catch {
                        continuation.resume(throwing: error)
                    }
                }
            }
        } onCancel: {
            controller.cancel()
        }
    }

    private func copyToRemoteDesktopBlocking(
        _ urls: [URL],
        timeout: TimeInterval,
        controller: CopyProcessController
    ) throws {
        guard !urls.isEmpty else {
            throw RemoteDesktopCopyError.noFilesSelected
        }

        let fileManager = FileManager.default
        for url in urls {
            guard url.isFileURL, fileManager.fileExists(atPath: url.path) else {
                throw RemoteDesktopCopyError.unavailableItem
            }
        }

        let itemNames = urls.map(\.lastPathComponent)
        guard Swift.Set(itemNames.map { $0.lowercased() }).count == itemNames.count else {
            throw RemoteDesktopCopyError.duplicateNames
        }

        let deadline = Date().addingTimeInterval(timeout)
        let stagingDirectory = ".lume-drop-\(UUID().uuidString)"
        var stagingNeedsCleanup = true
        defer {
            if stagingNeedsCleanup {
                cleanupStagingDirectory(stagingDirectory)
            }
        }
        let preparation = try executeCopyCommand(
            desktopPreparationCommand(
                itemNames: itemNames,
                stagingDirectory: stagingDirectory
            ),
            deadline: deadline,
            controller: controller
        )
        guard preparation.exitCode == 0 else {
            if preparation.exitCode == 73 {
                stagingNeedsCleanup = false
                throw RemoteDesktopCopyError.destinationExists
            }
            throw RemoteDesktopCopyError.preparationFailed(
                preparation.output.isEmpty ? nil : preparation.output
            )
        }
        let askpassPath = try createAskpassScript()
        defer { try? fileManager.removeItem(atPath: askpassPath) }

        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/scp")
        process.arguments = scpArguments(
            sourcePaths: urls.map(\.path),
            destinationDirectory: stagingDirectory
        )

        var environment = ProcessInfo.processInfo.environment
        environment["SSH_ASKPASS"] = askpassPath
        environment["SSH_ASKPASS_REQUIRE"] = "force"
        environment["DISPLAY"] = ":0"
        process.environment = environment
        process.standardInput = FileHandle.nullDevice
        process.standardOutput = FileHandle.nullDevice

        let stderrPipe = Pipe()
        process.standardError = stderrPipe

        try controller.register(process)
        do {
            try process.run()
        } catch {
            try? controller.finish(process)
            throw error
        }
        controller.processDidStart(process)
        controller.scheduleTimeout(for: process, deadline: deadline)
        let stderrData = stderrPipe.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        try controller.finish(process)

        guard process.terminationStatus == 0 else {
            let stderr = String(data: stderrData, encoding: .utf8)?
                .trimmingCharacters(in: .whitespacesAndNewlines)
            let message = stderr.flatMap { $0.isEmpty ? nil : $0 }
                ?? "SCP exited with code \(process.terminationStatus)"
            throw RemoteDesktopCopyError.transferFailed(message)
        }

        let commit = try executeCopyCommand(
            desktopCommitCommand(
                itemNames: itemNames,
                stagingDirectory: stagingDirectory
            ),
            deadline: deadline,
            controller: controller
        )
        guard commit.exitCode == 0 else {
            if commit.exitCode == 73 {
                throw RemoteDesktopCopyError.destinationExists
            }
            throw RemoteDesktopCopyError.commitFailed(
                commit.output.isEmpty ? nil : commit.output
            )
        }
        stagingNeedsCleanup = false
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

    func desktopPreparationCommand(
        itemNames: [String],
        stagingDirectory: String
    ) -> String {
        let checks = itemNames.map {
            "if [ -e \"$HOME/Desktop/\"\(Self.shellQuote($0)) ]; then exit 73; fi"
        }
        return (
            ["set -e", "mkdir -p \"$HOME/Desktop\""] + checks
                + ["mkdir \"$HOME/\(stagingDirectory)\""]
        ).joined(separator: "; ")
    }

    func desktopCommitCommand(itemNames: [String], stagingDirectory: String) -> String {
        let rollback = itemNames.map {
            let source = "\"$HOME/\(stagingDirectory)/\"\(Self.shellQuote($0))"
            let destination = "\"$HOME/Desktop/\"\(Self.shellQuote($0))"
            return "if [ ! -e \(source) ] && [ -e \(destination) ]; then "
                + "mv -n \(destination) \"$HOME/\(stagingDirectory)/\"; fi"
        }.joined(separator: "; ")
        let moves = itemNames.flatMap {
            let source = "\"$HOME/\(stagingDirectory)/\"\(Self.shellQuote($0))"
            let destination = "\"$HOME/Desktop/\"\(Self.shellQuote($0))"
            return [
                "if [ -e \(destination) ]; then exit 73; fi",
                "mv -n \(source) \"$HOME/Desktop/\"",
                "if [ -e \(source) ]; then exit 73; fi",
            ]
        }
        return (
            ["set -e", "rollback() { \(rollback); }", "trap rollback EXIT"] + moves
                + ["rmdir \"$HOME/\(stagingDirectory)\"", "trap - EXIT"]
        )
            .joined(separator: "; ")
    }

    func scpArguments(
        sourcePaths: [String],
        destinationDirectory: String
    ) -> [String] {
        var args = [
            "-q", "-r",
            "-o", "StrictHostKeyChecking=no",
            "-o", "UserKnownHostsFile=/dev/null",
            "-o", "LogLevel=ERROR",
            "-o", "ConnectTimeout=10",
            "-o", "ServerAliveInterval=15",
            "-o", "ServerAliveCountMax=4",
        ]

        if port != 22 {
            args += ["-P", "\(port)"]
        }

        args.append("--")
        args.append(contentsOf: sourcePaths)
        args.append("\(user)@\(host):\(destinationDirectory)/")
        return args
    }

    private func executeCopyCommand(
        _ command: String,
        deadline: Date,
        controller: CopyProcessController
    ) throws -> SSHResult {
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

        let outputPipe = Pipe()
        process.standardOutput = outputPipe
        process.standardError = outputPipe
        process.standardInput = FileHandle.nullDevice

        try controller.register(process)
        do {
            try process.run()
        } catch {
            try? controller.finish(process)
            throw error
        }
        controller.processDidStart(process)
        controller.scheduleTimeout(for: process, deadline: deadline)
        let outputData = outputPipe.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        try controller.finish(process)

        return SSHResult(
            exitCode: process.terminationStatus,
            output: Self.filteredSSHOutput(outputData)
        )
    }

    private func cleanupStagingDirectory(_ stagingDirectory: String) {
        let command = """
            if [ -d "$HOME/\(stagingDirectory)" ]; then \
            find "$HOME/\(stagingDirectory)" -depth -delete; \
            fi
            """
        _ = try? execute(command: command, timeout: 15)
    }

    private static func shellQuote(_ value: String) -> String {
        "'" + value.replacingOccurrences(of: "'", with: "'\\''") + "'"
    }

    private static func filteredSSHOutput(_ data: Data) -> String {
        let output = String(data: data, encoding: .utf8) ?? ""
        return output.components(separatedBy: "\n")
            .filter { line in
                !line.contains("Warning: Permanently added")
                    && !line.contains("known_hosts")
                    && !line.trimmingCharacters(in: .whitespaces).isEmpty
            }
            .joined(separator: "\n")
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
