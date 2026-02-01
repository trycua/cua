import AppKit
import Foundation

/// Watches the host clipboard and syncs changes to the VM via SSH.
/// Requires SSH/Remote Login to be enabled on the VM.
public actor ClipboardWatcher {
    private let vmName: String
    private let storage: String?
    private var lastChangeCount: Int = 0
    private var watchTask: Task<Void, Never>?
    private var isRunning = false

    /// Polling interval for clipboard changes
    private static let pollInterval: TimeInterval = 0.5

    /// Delay before starting to watch (allows VM to boot and get IP)
    private static let startupDelay: TimeInterval = 10.0

    public init(vmName: String, storage: String?) {
        self.vmName = vmName
        self.storage = storage
    }

    /// Start watching the host clipboard
    public func start() {
        guard !isRunning else { return }
        isRunning = true

        watchTask = Task { [weak self] in
            guard let self = self else { return }

            // Wait for VM to boot and potentially get an IP address
            try? await Task.sleep(nanoseconds: UInt64(Self.startupDelay * 1_000_000_000))

            // Initialize with current clipboard state to avoid syncing on start
            await self.updateLastChangeCount()

            Logger.info("Clipboard watcher started", metadata: ["vm": self.vmName])

            while !Task.isCancelled {
                await self.checkAndSync()
                try? await Task.sleep(nanoseconds: UInt64(Self.pollInterval * 1_000_000_000))
            }

            Logger.info("Clipboard watcher stopped", metadata: ["vm": self.vmName])
        }
    }

    /// Stop watching the clipboard
    public func stop() {
        watchTask?.cancel()
        watchTask = nil
        isRunning = false
    }

    private func updateLastChangeCount() {
        lastChangeCount = NSPasteboard.general.changeCount
    }

    private func checkAndSync() async {
        let currentChangeCount = NSPasteboard.general.changeCount

        guard currentChangeCount != lastChangeCount else {
            return // No change
        }

        lastChangeCount = currentChangeCount

        // Get clipboard content
        guard let content = NSPasteboard.general.string(forType: .string),
              !content.isEmpty else {
            return // No text content or empty
        }

        // Skip if content is too large (> 1MB)
        guard content.utf8.count <= 1_000_000 else {
            Logger.debug("Clipboard content too large to sync", metadata: [
                "size": "\(content.utf8.count)",
                "vm": vmName
            ])
            return
        }

        await syncToVM(content: content)
    }

    private func syncToVM(content: String) async {
        do {
            // Get VM details to find IP address (must be called on MainActor)
            let vmDetails = try await MainActor.run {
                let controller = LumeController()
                return try controller.getDetails(name: vmName, storage: storage)
            }

            guard vmDetails.status == "running" else {
                return // VM not running
            }

            guard let ipAddress = vmDetails.ipAddress, !ipAddress.isEmpty else {
                return // No IP address yet
            }

            guard vmDetails.sshAvailable == true else {
                return // SSH not available
            }

            // Create SSH client with default credentials
            let sshClient = SSHClient(
                host: ipAddress,
                port: 22,
                user: "lume",
                password: "lume"
            )

            // Base64 encode content for safe transport
            guard let data = content.data(using: .utf8) else { return }
            let base64Content = data.base64EncodedString()

            // Execute pbcopy on VM
            let command = "echo '\(base64Content)' | base64 -D | pbcopy"
            let result = try await sshClient.execute(command: command, timeout: 5)

            if result.exitCode == 0 {
                Logger.debug("Clipboard synced to VM", metadata: [
                    "vm": vmName,
                    "size": "\(content.utf8.count)"
                ])
            }
        } catch {
            // Silently ignore errors - SSH might not be ready yet
            Logger.debug("Failed to sync clipboard", metadata: [
                "vm": vmName,
                "error": error.localizedDescription
            ])
        }
    }
}
