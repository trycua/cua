import AppKit
import Foundation

/// Watches clipboard and syncs bidirectionally between host and VM via SSH.
/// Requires SSH/Remote Login to be enabled on the VM.
public actor ClipboardWatcher {
    private let vmName: String
    private let storage: String?
    private var watchTask: Task<Void, Never>?
    private var isRunning = false

    // Track last synced content to avoid sync loops
    private var lastHostContent: String = ""
    private var lastVMContent: String = ""
    private var lastHostChangeCount: Int = 0

    // Cached SSH client to avoid reconnecting every poll cycle
    private var cachedSSHClient: SSHClient?
    private var cachedIPAddress: String?

    // Error suppression to avoid flooding logs with repeated failures
    private var consecutiveFailures: Int = 0
    private var lastLoggedError: String?

    /// Polling interval for clipboard changes
    private static let pollInterval: TimeInterval = 1.0

    /// Backoff interval when SSH is unavailable
    private static let backoffInterval: TimeInterval = 5.0

    /// Number of consecutive failures before suppressing logs
    private static let errorLogThreshold: Int = 3

    /// Delay before starting to watch (allows VM to boot and get IP)
    /// Set to 0 since we gracefully handle SSH not being available yet
    private static let startupDelay: TimeInterval = 0

    /// Max content size (1MB)
    private static let maxContentSize = 1_000_000

    public init(vmName: String, storage: String?) {
        self.vmName = vmName
        self.storage = storage
    }

    /// Start watching clipboard for bidirectional sync
    public func start() {
        guard !isRunning else { return }
        isRunning = true

        watchTask = Task { [weak self] in
            guard let self = self else { return }

            // Optional startup delay (set to 0 since we gracefully handle SSH not ready)
            if Self.startupDelay > 0 {
                try? await Task.sleep(nanoseconds: UInt64(Self.startupDelay * 1_000_000_000))
            }

            // Initialize with current clipboard state to avoid syncing on start
            await self.initializeState()

            Logger.info("Clipboard sync started", metadata: ["vm": self.vmName])

            while !Task.isCancelled {
                await self.syncBidirectional()
                let interval = await self.currentPollInterval()
                try? await Task.sleep(nanoseconds: UInt64(interval * 1_000_000_000))
            }

            Logger.info("Clipboard sync stopped", metadata: ["vm": self.vmName])
        }
    }

    /// Stop watching the clipboard
    public func stop() {
        watchTask?.cancel()
        watchTask = nil
        isRunning = false
        cachedSSHClient = nil
        cachedIPAddress = nil
    }

    private func currentPollInterval() -> TimeInterval {
        consecutiveFailures >= Self.errorLogThreshold
            ? Self.backoffInterval
            : Self.pollInterval
    }

    private func initializeState() {
        lastHostChangeCount = NSPasteboard.general.changeCount
        if let content = NSPasteboard.general.string(forType: .string) {
            lastHostContent = content
            lastVMContent = content // Assume VM starts with same content
        }
    }

    private func syncBidirectional() async {
        // Check host clipboard for changes
        let hostChangeCount = NSPasteboard.general.changeCount
        var didSyncToVM = false

        if hostChangeCount != lastHostChangeCount {
            lastHostChangeCount = hostChangeCount
            if let content = NSPasteboard.general.string(forType: .string),
               !content.isEmpty,
               content != lastHostContent,
               content != lastVMContent,  // Avoid sync loop
               content.utf8.count <= Self.maxContentSize {
                lastHostContent = content
                lastVMContent = content  // Assume VM will have this content after sync
                didSyncToVM = await syncToVM(content: content)
            }
        }

        // Check VM clipboard for changes, but skip if we just synced to VM
        // (VM clipboard won't be updated yet, would cause race condition)
        if !didSyncToVM {
            await syncFromVM()
        }
    }

    private func syncToVM(content: String) async -> Bool {
        guard let sshClient = await getSSHClient() else { return false }

        do {
            guard let data = content.data(using: .utf8) else { return false }
            let base64Content = data.base64EncodedString()

            // For short content, use inline command
            // For long content, use heredoc to avoid shell argument limits
            let command: String
            if base64Content.count < 65536 {
                // Short content: inline is fine
                command = "printf '%s' '\(base64Content)' | base64 -D | pbcopy"
            } else {
                // Long content: use heredoc to avoid ARG_MAX limits
                command = """
                base64 -D <<'CLIPBOARD_EOF' | pbcopy
                \(base64Content)
                CLIPBOARD_EOF
                """
            }

            let result = try await sshClient.execute(command: command, timeout: 10)

            if result.exitCode == 0 {
                resetFailureState()
                Logger.debug("Clipboard synced to VM", metadata: [
                    "vm": vmName,
                    "size": "\(content.utf8.count)"
                ])
                return true
            }
            return false
        } catch {
            handleSyncError("Failed to sync clipboard to VM", error: error)
            return false
        }
    }

    private func syncFromVM() async {
        guard let sshClient = await getSSHClient() else { return }

        do {
            // Get VM clipboard content
            let result = try await sshClient.execute(command: "pbpaste | base64", timeout: 5)

            guard result.exitCode == 0 else { return }

            let trimmedOutput = result.output.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !trimmedOutput.isEmpty else { return }

            guard let data = Data(base64Encoded: trimmedOutput),
                  let content = String(data: data, encoding: .utf8),
                  !content.isEmpty,
                  content != lastVMContent,
                  content != lastHostContent,  // Avoid sync loop
                  content.utf8.count <= Self.maxContentSize else {
                return
            }

            // Update tracking
            lastVMContent = content

            // Write to host clipboard
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(content, forType: .string)
            lastHostChangeCount = NSPasteboard.general.changeCount
            lastHostContent = content

            resetFailureState()
            Logger.debug("Clipboard synced from VM", metadata: [
                "vm": vmName,
                "size": "\(content.utf8.count)"
            ])
        } catch {
            handleSyncError("Failed to sync clipboard from VM", error: error)
        }
    }

    /// Log sync errors with suppression to avoid flooding output.
    /// Logs the first occurrence and when the error changes, then suppresses
    /// repeated identical errors.
    private func handleSyncError(_ message: String, error: Error) {
        // Invalidate cached client on error so we reconnect next cycle
        cachedSSHClient = nil

        consecutiveFailures += 1
        let errorDesc = error.localizedDescription

        if errorDesc != lastLoggedError {
            // New error type — always log it
            Logger.debug(message, metadata: [
                "vm": vmName,
                "error": errorDesc
            ])
            lastLoggedError = errorDesc
        } else if consecutiveFailures == Self.errorLogThreshold {
            // Threshold reached — log once that we're suppressing
            Logger.debug("Clipboard sync errors repeating, suppressing further logs until resolved", metadata: [
                "vm": vmName
            ])
        }
        // Otherwise: same error, already suppressed — don't log
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

            // Reuse cached client if IP hasn't changed
            if let cached = cachedSSHClient, cachedIPAddress == ipAddress {
                return cached
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
