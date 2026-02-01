import Foundation

/// Clipboard synchronization service using SSH
public actor ClipboardSync {
    private let sshClient: SSHClient

    /// Maximum clipboard size (10MB)
    private static let maxClipboardSize = 10 * 1024 * 1024

    public init(sshClient: SSHClient) {
        self.sshClient = sshClient
    }

    /// Push host clipboard to VM
    /// - Returns: A message indicating success and bytes transferred
    public func push() async throws -> String {
        // 1. Read host clipboard using pbpaste
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/pbpaste")
        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = FileHandle.nullDevice

        do {
            try process.run()
            process.waitUntilExit()
        } catch {
            throw ClipboardError.hostClipboardReadFailed
        }

        let data = pipe.fileHandleForReading.readDataToEndOfFile()

        guard !data.isEmpty else {
            throw ClipboardError.hostClipboardEmpty
        }

        guard data.count <= Self.maxClipboardSize else {
            throw ClipboardError.contentTooLarge(data.count, Self.maxClipboardSize)
        }

        // Verify it's valid UTF-8 text
        guard String(data: data, encoding: .utf8) != nil else {
            throw ClipboardError.unsupportedContentType
        }

        // 2. Base64 encode for safe transport
        let base64Content = data.base64EncodedString()

        // 3. Execute on VM: decode and pipe to pbcopy
        // Use printf to avoid echo interpretation issues with special characters
        let command = "printf '%s' '\(base64Content)' | base64 -D | pbcopy"
        let result = try await sshClient.execute(command: command, timeout: 30)

        if result.exitCode != 0 {
            throw ClipboardError.vmCommandFailed(result.output)
        }

        return "Pushed \(data.count) bytes to VM clipboard"
    }

    /// Pull VM clipboard to host
    /// - Returns: A message indicating success and bytes transferred
    public func pull() async throws -> String {
        // 1. Read VM clipboard via SSH
        let command = "pbpaste | base64"
        let result = try await sshClient.execute(command: command, timeout: 30)

        guard result.exitCode == 0 else {
            throw ClipboardError.vmCommandFailed(result.output)
        }

        let trimmedOutput = result.output.trimmingCharacters(in: .whitespacesAndNewlines)

        guard !trimmedOutput.isEmpty else {
            throw ClipboardError.vmClipboardEmpty
        }

        // 2. Decode base64 content
        guard let data = Data(base64Encoded: trimmedOutput) else {
            throw ClipboardError.decodingFailed
        }

        guard data.count <= Self.maxClipboardSize else {
            throw ClipboardError.contentTooLarge(data.count, Self.maxClipboardSize)
        }

        // 3. Write to host clipboard using pbcopy
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/pbcopy")
        let inputPipe = Pipe()
        process.standardInput = inputPipe
        process.standardError = FileHandle.nullDevice

        do {
            try process.run()
            inputPipe.fileHandleForWriting.write(data)
            try inputPipe.fileHandleForWriting.close()
            process.waitUntilExit()
        } catch {
            throw ClipboardError.hostClipboardReadFailed
        }

        return "Pulled \(data.count) bytes from VM clipboard"
    }
}
