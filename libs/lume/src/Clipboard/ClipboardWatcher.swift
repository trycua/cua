import AppKit
import Foundation

enum ClipboardSyncError: LocalizedError {
    case unavailable
    case emptyClipboard
    case contentTooLarge
    case transferFailed

    var errorDescription: String? {
        switch self {
        case .unavailable:
            return "Clipboard sync is waiting for guest SSH access."
        case .emptyClipboard:
            return "The clipboard does not contain text or an image."
        case .contentTooLarge:
            return "The clipboard content is too large to transfer."
        case .transferFailed:
            return "The clipboard transfer failed."
        }
    }
}

private enum ClipboardPayload: Equatable, Sendable {
    case text(String)
    case png(Data)

    var byteCount: Int {
        switch self {
        case .text(let value):
            return value.utf8.count
        case .png(let data):
            return data.count
        }
    }

    var kind: String {
        switch self {
        case .text: return "text"
        case .png: return "image"
        }
    }
}

/// Watches text and image clipboards and syncs bidirectionally over SSH.
/// Requires SSH/Remote Login to be enabled on the VM.
public actor ClipboardWatcher {
    private let vmName: String
    private let storage: String?
    private var watchTask: Task<Void, Never>?
    private var isRunning = false

    private var lastHostPayload: ClipboardPayload?
    private var lastVMPayload: ClipboardPayload?
    private var pendingHostPayload: ClipboardPayload?
    private var lastHostChangeCount = 0
    private var lastVMChangeCount: Int?

    // Cached SSH client to avoid reconnecting every poll cycle.
    private var cachedSSHClient: SSHClient?
    private var cachedIPAddress: String?

    // Error suppression to avoid flooding logs with repeated failures.
    private var consecutiveFailures = 0
    private var lastLoggedError: String?

    private static let pollInterval: TimeInterval = 1.0
    private static let backoffInterval: TimeInterval = 5.0
    private static let errorLogThreshold = 3
    private static let maxTextSize = 1_000_000
    private static let maxImageSize = 10_000_000

    public init(vmName: String, storage: String?) {
        self.vmName = vmName
        self.storage = storage
    }

    public func start() {
        guard !isRunning else { return }
        isRunning = true

        watchTask = Task { [weak self] in
            guard let self else { return }
            await self.initializeState()
            Logger.info("Clipboard sync started", metadata: ["vm": self.vmName])

            while !Task.isCancelled {
                await self.syncBidirectional()
                let interval = await self.currentPollInterval()
                try? await Task.sleep(for: .seconds(interval))
            }

            Logger.info("Clipboard sync stopped", metadata: ["vm": self.vmName])
        }
    }

    public func stop() {
        watchTask?.cancel()
        watchTask = nil
        isRunning = false
        cachedSSHClient = nil
        cachedIPAddress = nil
        pendingHostPayload = nil
    }

    /// Immediately copies host text or image data into the guest clipboard.
    func pushHostClipboardToVM() async throws {
        let snapshot = await readHostClipboard()
        guard let payload = snapshot.payload else {
            throw ClipboardSyncError.emptyClipboard
        }
        try validateSize(of: payload)
        try await writePayloadToVM(payload)

        lastHostChangeCount = snapshot.changeCount
        lastHostPayload = payload
        lastVMPayload = payload
        pendingHostPayload = nil
        lastVMChangeCount = try? await readVMChangeCount()
    }

    /// Returns the current guest pasteboard generation for reliable Copy handling.
    func vmClipboardChangeCount() async throws -> Int {
        try await readVMChangeCount()
    }

    /// Immediately reads guest text or image data into the host clipboard.
    /// When a baseline is supplied, waits briefly for the guest's Copy command.
    func pullVMClipboardToHost(after baseline: Int? = nil) async throws {
        if let baseline {
            for _ in 0..<15 {
                let current = try await readVMChangeCount()
                if current != baseline {
                    break
                }
                try await Task.sleep(for: .milliseconds(100))
            }
        }

        let payload = try await readPayloadFromVM()
        try validateSize(of: payload)
        lastVMPayload = payload
        pendingHostPayload = nil
        lastVMChangeCount = try? await readVMChangeCount()
        lastHostChangeCount = await writeHostClipboard(payload)
        lastHostPayload = payload
        resetFailureState()
    }

    private func currentPollInterval() -> TimeInterval {
        consecutiveFailures >= Self.errorLogThreshold
            ? Self.backoffInterval
            : Self.pollInterval
    }

    private func initializeState() async {
        let snapshot = await readHostClipboard()
        lastHostChangeCount = snapshot.changeCount
        lastHostPayload = snapshot.payload
        // Do not overwrite either side merely because the VM has just started.
        lastVMPayload = snapshot.payload
    }

    private func syncBidirectional() async {
        let hostClipboard = await readHostClipboard()
        if hostClipboard.changeCount != lastHostChangeCount {
            lastHostChangeCount = hostClipboard.changeCount
            if let payload = hostClipboard.payload,
               payload != lastHostPayload,
               payload != lastVMPayload,
               isValidSize(payload) {
                lastHostPayload = payload
                pendingHostPayload = payload
            }
        }

        // Retry host changes until SSH accepts them. Do not pull stale guest content
        // while a host-to-guest transfer is pending.
        if let pendingHostPayload {
            do {
                try await writePayloadToVM(pendingHostPayload)
                lastVMPayload = pendingHostPayload
                self.pendingHostPayload = nil
                lastVMChangeCount = try? await readVMChangeCount()
                resetFailureState()
            } catch {
                handleSyncError("Failed to sync clipboard to VM", error: error)
            }
            return
        }

        do {
            let currentChangeCount = try await readVMChangeCount()
            guard let previousChangeCount = lastVMChangeCount else {
                lastVMChangeCount = currentChangeCount
                return
            }
            guard currentChangeCount != previousChangeCount else { return }
            lastVMChangeCount = currentChangeCount

            let payload = try await readPayloadFromVM()
            guard payload != lastVMPayload,
                  payload != lastHostPayload,
                  isValidSize(payload) else {
                return
            }

            lastVMPayload = payload
            lastHostChangeCount = await writeHostClipboard(payload)
            lastHostPayload = payload
            resetFailureState()
            Logger.debug("Clipboard synced from VM", metadata: [
                "vm": vmName,
                "kind": payload.kind,
                "size": "\(payload.byteCount)",
            ])
        } catch {
            handleSyncError("Failed to sync clipboard from VM", error: error)
        }
    }

    private func writePayloadToVM(_ payload: ClipboardPayload) async throws {
        guard let sshClient = await getSSHClient() else {
            throw ClipboardSyncError.unavailable
        }

        let command: String
        let timeout: TimeInterval
        switch payload {
        case .text(let content):
            guard let data = content.data(using: .utf8) else {
                throw ClipboardSyncError.transferFailed
            }
            let encoded = data.base64EncodedString()
            if encoded.count < 65_536 {
                command = "printf '%s' '\(encoded)' | base64 -D | pbcopy"
            } else {
                command = """
                    base64 -D <<'LUME_CLIPBOARD_EOF' | pbcopy
                    \(encoded)
                    LUME_CLIPBOARD_EOF
                    """
            }
            timeout = 10

        case .png(let data):
            let encoded = data.base64EncodedString()
            let path = "/tmp/lume-clipboard-\(UUID().uuidString).png"
            command = """
                base64 -D <<'LUME_CLIPBOARD_EOF' > '\(path)'
                \(encoded)
                LUME_CLIPBOARD_EOF
                /usr/bin/osascript -e 'set the clipboard to (read (POSIX file "\(path)") as «class PNGf»)'
                lume_status=$?
                rm -f '\(path)'
                exit $lume_status
                """
            timeout = 30
        }

        let result = try await sshClient.execute(command: command, timeout: timeout)
        guard result.exitCode == 0 else {
            throw ClipboardSyncError.transferFailed
        }

        resetFailureState()
        Logger.debug("Clipboard synced to VM", metadata: [
            "vm": vmName,
            "kind": payload.kind,
            "size": "\(payload.byteCount)",
        ])
    }

    private func readVMChangeCount() async throws -> Int {
        guard let sshClient = await getSSHClient() else {
            throw ClipboardSyncError.unavailable
        }
        let command =
            "osascript -l JavaScript -e 'ObjC.import(\"AppKit\"); $.NSPasteboard.generalPasteboard.changeCount'"
        let result = try await sshClient.execute(command: command, timeout: 5)
        guard result.exitCode == 0,
              let value = Int(result.output.trimmingCharacters(in: .whitespacesAndNewlines)) else {
            throw ClipboardSyncError.transferFailed
        }
        return value
    }

    private func readPayloadFromVM() async throws -> ClipboardPayload {
        guard let sshClient = await getSSHClient() else {
            throw ClipboardSyncError.unavailable
        }

        let path = "/tmp/lume-clipboard-\(UUID().uuidString).png"
        let command = """
            if /usr/bin/osascript \\
              -e 'set imageData to the clipboard as «class PNGf»' \\
              -e 'set fileRef to open for access POSIX file "\(path)" with write permission' \\
              -e 'set eof fileRef to 0' \\
              -e 'write imageData to fileRef' \\
              -e 'close access fileRef' >/dev/null 2>&1; then
              printf 'IMAGE\\n'
              base64 < '\(path)'
            else
              printf 'TEXT\\n'
              pbpaste | base64
            fi
            rm -f '\(path)'
            """

        let result = try await sshClient.execute(command: command, timeout: 30)
        guard result.exitCode == 0,
              let separator = result.output.firstIndex(of: "\n") else {
            throw ClipboardSyncError.transferFailed
        }

        let kind = result.output[..<separator].trimmingCharacters(in: .whitespacesAndNewlines)
        let encoded = String(result.output[result.output.index(after: separator)...])
        guard let data = Data(base64Encoded: encoded, options: .ignoreUnknownCharacters),
              !data.isEmpty else {
            throw ClipboardSyncError.emptyClipboard
        }

        switch kind {
        case "IMAGE":
            return .png(data)
        case "TEXT":
            guard let content = String(data: data, encoding: .utf8), !content.isEmpty else {
                throw ClipboardSyncError.emptyClipboard
            }
            return .text(content)
        default:
            throw ClipboardSyncError.transferFailed
        }
    }

    private func readHostClipboard() async -> (changeCount: Int, payload: ClipboardPayload?) {
        await MainActor.run {
            let pasteboard = NSPasteboard.general
            let payload: ClipboardPayload?

            if let png = pasteboard.data(forType: .png), !png.isEmpty {
                payload = .png(png)
            } else if let tiff = pasteboard.data(forType: .tiff),
                      let representation = NSBitmapImageRep(data: tiff),
                      let png = representation.representation(using: .png, properties: [:]) {
                payload = .png(png)
            } else if let content = pasteboard.string(forType: .string), !content.isEmpty {
                payload = .text(content)
            } else {
                payload = nil
            }
            return (pasteboard.changeCount, payload)
        }
    }

    private func writeHostClipboard(_ payload: ClipboardPayload) async -> Int {
        await MainActor.run {
            let pasteboard = NSPasteboard.general
            pasteboard.clearContents()
            switch payload {
            case .text(let content):
                pasteboard.setString(content, forType: .string)
            case .png(let data):
                pasteboard.setData(data, forType: .png)
                if let image = NSImage(data: data), let tiff = image.tiffRepresentation {
                    pasteboard.setData(tiff, forType: .tiff)
                }
            }
            return pasteboard.changeCount
        }
    }

    private func validateSize(of payload: ClipboardPayload) throws {
        guard isValidSize(payload) else {
            throw ClipboardSyncError.contentTooLarge
        }
    }

    private func isValidSize(_ payload: ClipboardPayload) -> Bool {
        switch payload {
        case .text:
            return payload.byteCount <= Self.maxTextSize
        case .png:
            return payload.byteCount <= Self.maxImageSize
        }
    }

    private func handleSyncError(_ message: String, error: Error) {
        cachedSSHClient = nil
        consecutiveFailures += 1
        let errorDescription = error.localizedDescription

        if errorDescription != lastLoggedError {
            Logger.debug(message, metadata: [
                "vm": vmName,
                "error": errorDescription,
            ])
            lastLoggedError = errorDescription
        } else if consecutiveFailures == Self.errorLogThreshold {
            Logger.debug(
                "Clipboard sync errors repeating, suppressing further logs until resolved",
                metadata: ["vm": vmName]
            )
        }
    }

    private func resetFailureState() {
        if consecutiveFailures >= Self.errorLogThreshold {
            Logger.debug("Clipboard sync recovered", metadata: ["vm": vmName])
        }
        consecutiveFailures = 0
        lastLoggedError = nil
    }

    private func getSSHClient() async -> SSHClient? {
        do {
            let vmDetails = try await MainActor.run {
                let controller = LumeController()
                return try controller.getDetails(name: vmName, storage: storage)
            }

            guard vmDetails.status == "running",
                  let ipAddress = vmDetails.ipAddress, !ipAddress.isEmpty,
                  vmDetails.sshAvailable == true else {
                return nil
            }

            if let cachedSSHClient, cachedIPAddress == ipAddress {
                return cachedSSHClient
            }

            let client = SSHClient(
                host: ipAddress,
                port: 22,
                user: "lume",
                password: "lume"
            )
            cachedSSHClient = client
            cachedIPAddress = ipAddress
            return client
        } catch {
            return nil
        }
    }
}
