import Foundation
import CoreGraphics

// MARK: - Support Types

/// Base context for virtual machine directory and configuration
struct VMDirContext {
    let dir: VMDirectory
    var config: VMConfig
    let home: Home
    let storage: String?

    func saveConfig() throws {
        try dir.saveConfig(config)
    }

    var name: String { dir.name }
    var initialized: Bool { dir.initialized() }
    var diskPath: Path { dir.diskPath }
    var nvramPath: Path { dir.nvramPath }

    func setDisk(_ size: UInt64) throws {
        try dir.setDisk(size)
    }

    func finalize(to name: String) throws {
        let vmDir = try home.getVMDirectory(name)
        try FileManager.default.moveItem(at: dir.dir.url, to: vmDir.dir.url)
    }
}

// MARK: - Base VM Class

/// Base class for virtual machine implementations
@MainActor
class VM {
    // MARK: - Properties

    var vmDirContext: VMDirContext

    @MainActor
    private var virtualizationService: VMVirtualizationService?
    internal let vncService: VNCService
    private var clipboardWatcher: ClipboardWatcher?
    internal let virtualizationServiceFactory:
        (VMVirtualizationServiceContext) throws -> VMVirtualizationService
    private let vncServiceFactory: (VMDirectory) -> VNCService

    // MARK: - Initialization

    init(
        vmDirContext: VMDirContext,
        virtualizationServiceFactory: @escaping (VMVirtualizationServiceContext) throws ->
            VMVirtualizationService = { try DarwinVirtualizationService(configuration: $0) },
        vncServiceFactory: @escaping (VMDirectory) -> VNCService = {
            DefaultVNCService(vmDirectory: $0)
        }
    ) {
        self.vmDirContext = vmDirContext
        self.virtualizationServiceFactory = virtualizationServiceFactory
        self.vncServiceFactory = vncServiceFactory

        // Initialize VNC service
        self.vncService = vncServiceFactory(vmDirContext.dir)
    }

    // MARK: - Public Accessors

    /// The VM name
    var name: String { vmDirContext.name }

    /// The VM configuration
    var config: VMConfig { vmDirContext.config }

    // MARK: - VM State Management

    private var isRunning: Bool {
        // First check if we have a MAC address
        guard let macAddress = vmDirContext.config.macAddress else {
            Logger.info(
                "Cannot check if VM is running: macAddress is nil",
                metadata: ["name": vmDirContext.name])
            return false
        }

        // Then check if we have an IP address
        guard let ipAddress = DHCPLeaseParser.getIPAddress(forMAC: macAddress) else {
            return false
        }

        // Then check if it's reachable
        return NetworkUtils.isReachable(ipAddress: ipAddress)
    }

    var details: VMDetails {
        let isRunning: Bool = self.isRunning
        let vncUrl = isRunning ? getVNCUrl() : nil

        // Safely get disk size with fallback
        let diskSizeValue: DiskSize
        do {
            diskSizeValue = try getDiskSize()
        } catch {
            Logger.error(
                "Failed to get disk size",
                metadata: ["name": vmDirContext.name, "error": "\(error)"])
            // Provide a fallback value to avoid crashing
            diskSizeValue = DiskSize(allocated: 0, total: vmDirContext.config.diskSize ?? 0)
        }

        // Safely access MAC address
        let macAddress = vmDirContext.config.macAddress
        let ipAddress: String? =
            isRunning && macAddress != nil ? DHCPLeaseParser.getIPAddress(forMAC: macAddress!) : nil

        // Check if SSH is available (only if we have an IP)
        let sshAvailable: Bool? = ipAddress != nil ? NetworkUtils.isSSHAvailable(ipAddress: ipAddress!) : nil

        return VMDetails(
            name: vmDirContext.name,
            os: getOSType(),
            cpuCount: vmDirContext.config.cpuCount ?? 0,
            memorySize: vmDirContext.config.memorySize ?? 0,
            diskSize: diskSizeValue,
            display: vmDirContext.config.display.string,
            status: isRunning ? "running" : "stopped",
            vncUrl: vncUrl,
            ipAddress: ipAddress,
            sshAvailable: sshAvailable,
            locationName: vmDirContext.storage ?? "home",
            networkMode: vmDirContext.config.networkMode.description
        )
    }

    // MARK: - VM Lifecycle Management

    func run(
        noDisplay: Bool, sharedDirectories: [SharedDirectory], mount: Path?, vncPort: Int = 0,
        recoveryMode: Bool = false, usbMassStoragePaths: [Path]? = nil,
        networkMode: NetworkMode? = nil, clipboard: Bool = false
    ) async throws {
        Logger.info(
            "VM.run method called",
            metadata: [
                "name": vmDirContext.name,
                "noDisplay": "\(noDisplay)",
                "recoveryMode": "\(recoveryMode)",
            ])

        guard vmDirContext.initialized else {
            Logger.error("VM not initialized", metadata: ["name": vmDirContext.name])
            throw VMError.notInitialized(vmDirContext.name)
        }

        guard let cpuCount = vmDirContext.config.cpuCount,
            let memorySize = vmDirContext.config.memorySize
        else {
            Logger.error("VM missing cpuCount or memorySize", metadata: ["name": vmDirContext.name])
            throw VMError.notInitialized(vmDirContext.name)
        }

        // Try to acquire lock on config file
        Logger.info(
            "Attempting to acquire lock on config file",
            metadata: [
                "path": vmDirContext.dir.configPath.path,
                "name": vmDirContext.name,
            ])
        var fileHandle = try FileHandle(forWritingTo: vmDirContext.dir.configPath.url)

        if flock(fileHandle.fileDescriptor, LOCK_EX | LOCK_NB) != 0 {
            try? fileHandle.close()
            Logger.error(
                "VM already running (failed to acquire lock)", metadata: ["name": vmDirContext.name]
            )

            // Try to forcibly clear the lock before giving up
            Logger.info("Attempting emergency lock cleanup", metadata: ["name": vmDirContext.name])
            unlockConfigFile()

            // Try one more time to acquire the lock
            if let retryHandle = try? FileHandle(forWritingTo: vmDirContext.dir.configPath.url),
                flock(retryHandle.fileDescriptor, LOCK_EX | LOCK_NB) == 0
            {
                Logger.info("Emergency lock cleanup worked", metadata: ["name": vmDirContext.name])
                // Continue with a fresh file handle
                try? retryHandle.close()
                // Get a completely new file handle to be safe
                guard let newHandle = try? FileHandle(forWritingTo: vmDirContext.dir.configPath.url)
                else {
                    throw VMError.internalError("Failed to open file handle after lock cleanup")
                }
                // Update our main file handle
                fileHandle = newHandle
            } else {
                // If we still can't get the lock, give up
                Logger.error(
                    "Could not acquire lock even after emergency cleanup",
                    metadata: ["name": vmDirContext.name])
                throw VMError.alreadyRunning(vmDirContext.name)
            }
        }
        Logger.info("Successfully acquired lock", metadata: ["name": vmDirContext.name])

        Logger.info(
            "Running VM with configuration",
            metadata: [
                "name": vmDirContext.name,
                "cpuCount": "\(cpuCount)",
                "memorySize": "\(memorySize)",
                "diskSize": "\(vmDirContext.config.diskSize ?? 0)",
                "sharedDirectories": sharedDirectories.map { $0.string }.joined(separator: ", "),
                "recoveryMode": "\(recoveryMode)",
            ])

        // Create and configure the VM
        do {
            Logger.info(
                "Creating virtualization service context", metadata: ["name": vmDirContext.name])
            let config = try createVMVirtualizationServiceContext(
                cpuCount: cpuCount,
                memorySize: memorySize,
                display: vmDirContext.config.display.string,
                sharedDirectories: sharedDirectories,
                mount: mount,
                recoveryMode: recoveryMode,
                usbMassStoragePaths: usbMassStoragePaths,
                networkMode: networkMode
            )
            Logger.info(
                "Successfully created virtualization service context",
                metadata: ["name": vmDirContext.name])

            Logger.info(
                "Initializing virtualization service", metadata: ["name": vmDirContext.name])
            virtualizationService = try virtualizationServiceFactory(config)
            Logger.info(
                "Successfully initialized virtualization service",
                metadata: ["name": vmDirContext.name])

            Logger.info(
                "Setting up VNC",
                metadata: [
                    "name": vmDirContext.name,
                    "noDisplay": "\(noDisplay)",
                    "port": "\(vncPort)",
                ])
            let vncInfo = try await setupSession(
                port: vncPort, sharedDirectories: sharedDirectories)
            Logger.info(
                "VNC setup successful", metadata: ["name": vmDirContext.name, "vncInfo": vncInfo])

            // Start the VM
            guard let service = virtualizationService else {
                Logger.error("Virtualization service is nil", metadata: ["name": vmDirContext.name])
                throw VMError.internalError("Virtualization service not initialized")
            }
            Logger.info(
                "Starting VM via virtualization service", metadata: ["name": vmDirContext.name])
            try await service.start()
            Logger.info("VM started successfully", metadata: ["name": vmDirContext.name])

            // Open the VNC client only after VM start to avoid connecting to an empty framebuffer.
            if !noDisplay {
                await waitForVisibleFramebufferBeforeOpeningClient()
                Logger.info("Starting VNC session", metadata: ["name": vmDirContext.name])
                try await vncService.openClient(url: vncInfo)
            }

            // Start clipboard watcher for automatic host-to-VM clipboard sync
            // Requires SSH/Remote Login to be enabled on the VM
            if clipboard {
                clipboardWatcher = ClipboardWatcher(vmName: vmDirContext.name, storage: vmDirContext.storage)
                await clipboardWatcher?.start()
            }

            while true {
                try await Task.sleep(nanoseconds: UInt64(1e9))
            }
        } catch {
            Logger.error(
                "Failed in VM.run",
                metadata: [
                    "name": vmDirContext.name,
                    "error": error.localizedDescription,
                    "errorType": "\(type(of: error))",
                ])
            await clipboardWatcher?.stop()
            clipboardWatcher = nil
            virtualizationService = nil
            vncService.stop()

            // Release lock
            Logger.info("Releasing file lock after error", metadata: ["name": vmDirContext.name])
            flock(fileHandle.fileDescriptor, LOCK_UN)
            try? fileHandle.close()

            // Additionally, perform our aggressive unlock to ensure no locks remain
            Logger.info(
                "Performing additional lock cleanup after error",
                metadata: ["name": vmDirContext.name])
            unlockConfigFile()

            throw error
        }
    }

    @MainActor
    func stop() async throws {
        guard vmDirContext.initialized else {
            throw VMError.notInitialized(vmDirContext.name)
        }

        Logger.info("Attempting to stop VM", metadata: ["name": vmDirContext.name])

        // If we have a virtualization service, try to stop it cleanly first
        if let service = virtualizationService {
            do {
                Logger.info(
                    "Stopping VM via virtualization service", metadata: ["name": vmDirContext.name])
                try await service.stop()
                await clipboardWatcher?.stop()
                clipboardWatcher = nil
                virtualizationService = nil
                vncService.stop()
                Logger.info(
                    "VM stopped successfully via virtualization service",
                    metadata: ["name": vmDirContext.name])

                // Try to ensure any existing locks are released
                Logger.info(
                    "Attempting to clear any locks on config file",
                    metadata: ["name": vmDirContext.name])
                unlockConfigFile()

                return
            } catch let error {
                Logger.error(
                    "Failed to stop VM via virtualization service",
                    metadata: [
                        "name": vmDirContext.name,
                        "error": error.localizedDescription,
                    ])
                // Fall through to process termination
            }
        }

        // Try to open config file to get file descriptor
        Logger.info(
            "Attempting to access config file lock",
            metadata: [
                "path": vmDirContext.dir.configPath.path,
                "name": vmDirContext.name,
            ])
        let fileHandle = try? FileHandle(forReadingFrom: vmDirContext.dir.configPath.url)
        guard let fileHandle = fileHandle else {
            Logger.info(
                "Failed to open config file - VM may not be running",
                metadata: ["name": vmDirContext.name])

            // Even though we couldn't open the file, try to force unlock anyway
            unlockConfigFile()

            throw VMError.notRunning(vmDirContext.name)
        }

        // Get the PID of the process holding the lock using lsof command
        Logger.info(
            "Finding process holding lock on config file", metadata: ["name": vmDirContext.name])
        let task = Process()
        task.executableURL = URL(fileURLWithPath: "/usr/sbin/lsof")
        task.arguments = ["-F", "p", vmDirContext.dir.configPath.path]

        let outputPipe = Pipe()
        task.standardOutput = outputPipe

        try task.run()
        task.waitUntilExit()

        let outputData = try outputPipe.fileHandleForReading.readToEnd() ?? Data()
        guard let outputString = String(data: outputData, encoding: .utf8),
            let pidString = outputString.split(separator: "\n").first?.dropFirst(),  // Drop the 'p' prefix
            let pid = pid_t(pidString)
        else {
            try? fileHandle.close()
            Logger.info(
                "Failed to find process holding lock - VM may not be running",
                metadata: ["name": vmDirContext.name])

            // Even though we couldn't find the process, try to force unlock
            unlockConfigFile()

            throw VMError.notRunning(vmDirContext.name)
        }

        Logger.info(
            "Found process \(pid) holding lock on config file",
            metadata: ["name": vmDirContext.name])

        // First try graceful shutdown with SIGINT
        if kill(pid, SIGINT) == 0 {
            Logger.info("Sent SIGINT to VM process \(pid)", metadata: ["name": vmDirContext.name])
        }

        // Wait for process to stop with timeout
        var attempts = 0
        while attempts < 10 {
            Logger.info(
                "Waiting for process \(pid) to terminate (attempt \(attempts + 1)/10)",
                metadata: ["name": vmDirContext.name])
            try await Task.sleep(nanoseconds: 1_000_000_000)

            // Check if process still exists
            if kill(pid, 0) != 0 {
                // Process is gone, do final cleanup
                Logger.info("Process \(pid) has terminated", metadata: ["name": vmDirContext.name])
                virtualizationService = nil
                vncService.stop()
                try? fileHandle.close()

                // Force unlock the config file
                unlockConfigFile()

                Logger.info(
                    "VM stopped successfully via process termination",
                    metadata: ["name": vmDirContext.name])
                return
            }
            attempts += 1
        }

        // If graceful shutdown failed, force kill the process
        Logger.info(
            "Graceful shutdown failed, forcing termination of process \(pid)",
            metadata: ["name": vmDirContext.name])
        if kill(pid, SIGKILL) == 0 {
            Logger.info("Sent SIGKILL to process \(pid)", metadata: ["name": vmDirContext.name])

            // Wait a moment for the process to be fully killed
            try await Task.sleep(nanoseconds: 2_000_000_000)

            // Do final cleanup
            virtualizationService = nil
            vncService.stop()
            try? fileHandle.close()

            // Force unlock the config file
            unlockConfigFile()

            Logger.info("VM forcefully stopped", metadata: ["name": vmDirContext.name])
            return
        }

        // If we get here, something went very wrong
        try? fileHandle.close()
        Logger.error(
            "Failed to stop VM - could not terminate process \(pid)",
            metadata: ["name": vmDirContext.name])

        // As a last resort, try to force unlock
        unlockConfigFile()

        throw VMError.internalError("Failed to stop VM process")
    }

    // Helper method to forcibly clear any locks on the config file
    private func unlockConfigFile() {
        Logger.info(
            "Forcibly clearing locks on config file",
            metadata: [
                "path": vmDirContext.dir.configPath.path,
                "name": vmDirContext.name,
            ])

        // First attempt: standard unlock methods
        if let fileHandle = try? FileHandle(forWritingTo: vmDirContext.dir.configPath.url) {
            // Use F_GETLK and F_SETLK to check and clear locks
            var lockInfo = flock()
            lockInfo.l_type = Int16(F_UNLCK)
            lockInfo.l_whence = Int16(SEEK_SET)
            lockInfo.l_start = 0
            lockInfo.l_len = 0

            // Try to unlock the file using fcntl
            _ = fcntl(fileHandle.fileDescriptor, F_SETLK, &lockInfo)

            // Also try the regular flock method
            flock(fileHandle.fileDescriptor, LOCK_UN)

            try? fileHandle.close()
            Logger.info("Standard unlock attempts performed", metadata: ["name": vmDirContext.name])
        }

        // Second attempt: try to acquire and immediately release a fresh lock
        if let tempHandle = try? FileHandle(forWritingTo: vmDirContext.dir.configPath.url) {
            if flock(tempHandle.fileDescriptor, LOCK_EX | LOCK_NB) == 0 {
                Logger.info(
                    "Successfully acquired and released lock to reset state",
                    metadata: ["name": vmDirContext.name])
                flock(tempHandle.fileDescriptor, LOCK_UN)
            } else {
                Logger.info(
                    "Could not acquire lock for resetting - may still be locked",
                    metadata: ["name": vmDirContext.name])
            }
            try? tempHandle.close()
        }

        // Third attempt (most aggressive): copy the config file, remove the original, and restore
        Logger.info(
            "Trying aggressive method: backup and restore config file",
            metadata: ["name": vmDirContext.name])
        // Only proceed if the config file exists
        let fileManager = FileManager.default
        let configPath = vmDirContext.dir.configPath.path
        let backupPath = configPath + ".backup"

        if fileManager.fileExists(atPath: configPath) {
            // Create a backup of the config file
            if let configData = try? Data(contentsOf: URL(fileURLWithPath: configPath)) {
                // Make backup
                try? configData.write(to: URL(fileURLWithPath: backupPath))

                // Remove the original file to clear all locks
                try? fileManager.removeItem(atPath: configPath)
                Logger.info(
                    "Removed original config file to clear locks",
                    metadata: ["name": vmDirContext.name])

                // Wait a moment for OS to fully release resources
                Thread.sleep(forTimeInterval: 0.1)

                // Restore from backup
                try? configData.write(to: URL(fileURLWithPath: configPath))
                Logger.info(
                    "Restored config file from backup", metadata: ["name": vmDirContext.name])
            } else {
                Logger.error(
                    "Could not read config file content for backup",
                    metadata: ["name": vmDirContext.name])
            }
        } else {
            Logger.info(
                "Config file does not exist, cannot perform aggressive unlock",
                metadata: ["name": vmDirContext.name])
        }

        // Final check
        if let finalHandle = try? FileHandle(forWritingTo: vmDirContext.dir.configPath.url) {
            let lockResult = flock(finalHandle.fileDescriptor, LOCK_EX | LOCK_NB)
            if lockResult == 0 {
                Logger.info(
                    "Lock successfully cleared - verified by acquiring test lock",
                    metadata: ["name": vmDirContext.name])
                flock(finalHandle.fileDescriptor, LOCK_UN)
            } else {
                Logger.info(
                    "Lock still present after all clearing attempts",
                    metadata: ["name": vmDirContext.name, "severity": "warning"])
            }
            try? finalHandle.close()
        }
    }

    // MARK: - Resource Management

    func updateVMConfig(vmConfig: VMConfig) throws {
        vmDirContext.config = vmConfig
        try vmDirContext.saveConfig()
    }

    private func getDiskSize() throws -> DiskSize {
        let resourceValues = try vmDirContext.diskPath.url.resourceValues(forKeys: [
            .totalFileAllocatedSizeKey,
            .totalFileSizeKey,
        ])

        guard let allocated = resourceValues.totalFileAllocatedSize,
            let total = resourceValues.totalFileSize
        else {
            throw VMConfigError.invalidDiskSize
        }

        return DiskSize(allocated: UInt64(allocated), total: UInt64(total))
    }

    func resizeDisk(_ newSize: UInt64) throws {
        let currentSize = try getDiskSize()

        guard newSize >= currentSize.total else {
            throw VMError.resizeTooSmall(current: currentSize.total, requested: newSize)
        }

        try setDiskSize(newSize)
    }

    func setCpuCount(_ newCpuCount: Int) throws {
        guard !isRunning else {
            throw VMError.alreadyRunning(vmDirContext.name)
        }
        vmDirContext.config.setCpuCount(newCpuCount)
        try vmDirContext.saveConfig()
    }

    func setMemorySize(_ newMemorySize: UInt64) throws {
        guard !isRunning else {
            throw VMError.alreadyRunning(vmDirContext.name)
        }
        vmDirContext.config.setMemorySize(newMemorySize)
        try vmDirContext.saveConfig()
    }

    func setDiskSize(_ newDiskSize: UInt64) throws {
        try vmDirContext.setDisk(newDiskSize)
        vmDirContext.config.setDiskSize(newDiskSize)
        try vmDirContext.saveConfig()
    }

    func setDisplay(_ newDisplay: String) throws {
        guard !isRunning else {
            throw VMError.alreadyRunning(vmDirContext.name)
        }
        guard let display: VMDisplayResolution = VMDisplayResolution(string: newDisplay) else {
            throw VMError.invalidDisplayResolution(newDisplay)
        }
        vmDirContext.config.setDisplay(display)
        try vmDirContext.saveConfig()
    }

    func setHardwareModel(_ newHardwareModel: Data) throws {
        guard !isRunning else {
            throw VMError.alreadyRunning(vmDirContext.name)
        }
        vmDirContext.config.setHardwareModel(newHardwareModel)
        try vmDirContext.saveConfig()
    }

    func setMachineIdentifier(_ newMachineIdentifier: Data) throws {
        guard !isRunning else {
            throw VMError.alreadyRunning(vmDirContext.name)
        }
        vmDirContext.config.setMachineIdentifier(newMachineIdentifier)
        try vmDirContext.saveConfig()
    }

    func setMacAddress(_ newMacAddress: String) throws {
        guard !isRunning else {
            throw VMError.alreadyRunning(vmDirContext.name)
        }
        vmDirContext.config.setMacAddress(newMacAddress)
        try vmDirContext.saveConfig()
    }

    // MARK: - VNC Management

    func getVNCUrl() -> String? {
        return vncService.url
    }

    /// Sets up the VNC service and returns the VNC URL
    private func startVNCService(port: Int = 0) async throws -> String {
        guard let service = virtualizationService else {
            throw VMError.internalError("Virtualization service not initialized")
        }

        try await vncService.start(port: port, virtualMachine: service.getVirtualMachine())

        guard let url = vncService.url else {
            throw VMError.vncNotConfigured
        }

        return url
    }

    /// Saves the session information including shared directories to disk
    private func saveSessionData(url: String, sharedDirectories: [SharedDirectory]) {
        do {
            let session = VNCSession(
                url: url, sharedDirectories: sharedDirectories.isEmpty ? nil : sharedDirectories)
            try vmDirContext.dir.saveSession(session)
            Logger.info(
                "Saved VNC session with shared directories",
                metadata: [
                    "count": "\(sharedDirectories.count)",
                    "dirs": "\(sharedDirectories.map { $0.hostPath }.joined(separator: ", "))",
                    "sessionsPath": "\(vmDirContext.dir.sessionsPath.path)",
                ])
        } catch {
            Logger.error("Failed to save VNC session", metadata: ["error": "\(error)"])
        }
    }

    /// Main session setup method that handles VNC and persists session data
    private func setupSession(
        port: Int = 0, sharedDirectories: [SharedDirectory] = []
    ) async throws -> String {
        // Start the VNC service and get the URL
        let url = try await startVNCService(port: port)

        // Save the session data
        saveSessionData(url: url, sharedDirectories: sharedDirectories)

        return url
    }

    /// Avoid opening Screen Sharing on an all-black initial framebuffer.
    /// This only runs for the real VNC service (not mocks in tests).
    private func waitForVisibleFramebufferBeforeOpeningClient() async {
        guard vncService is DefaultVNCService else {
            return
        }

        do {
            try await vncService.connectInputClient()
            defer { vncService.disconnectInputClient() }

            let timeoutSeconds = 30
            for _ in 0..<timeoutSeconds {
                if let image = try? await vncService.captureFramebuffer(),
                    framebufferHasVisiblePixels(image)
                {
                    Logger.info(
                        "Detected visible VM framebuffer content before opening VNC client",
                        metadata: ["name": vmDirContext.name]
                    )
                    return
                }
                try? await Task.sleep(nanoseconds: 1_000_000_000)
            }

            Logger.info(
                "Timed out waiting for visible framebuffer content; opening VNC client anyway",
                metadata: ["name": vmDirContext.name, "timeout_seconds": "\(timeoutSeconds)"]
            )
        } catch {
            Logger.info(
                "Framebuffer readiness check failed; opening VNC client anyway",
                metadata: ["name": vmDirContext.name, "error": "\(error)"]
            )
        }
    }

    /// Fast heuristic: sample bytes from the framebuffer and treat any non-zero value as visible content.
    private func framebufferHasVisiblePixels(_ image: CGImage) -> Bool {
        guard let dataProvider = image.dataProvider,
            let data = dataProvider.data,
            let bytes = CFDataGetBytePtr(data)
        else {
            // If we can't inspect pixels, do not block client opening.
            return true
        }

        let count = CFDataGetLength(data)
        guard count > 0 else {
            return false
        }

        let stride = max(1, count / 4096)
        var index = 0
        while index < count {
            if bytes[index] != 0 {
                return true
            }
            index += stride
        }

        return false
    }

    // MARK: - Platform-specific Methods

    func getOSType() -> String {
        fatalError("Must be implemented by subclass")
    }

    func createVMVirtualizationServiceContext(
        cpuCount: Int,
        memorySize: UInt64,
        display: String,
        sharedDirectories: [SharedDirectory] = [],
        mount: Path? = nil,
        recoveryMode: Bool = false,
        usbMassStoragePaths: [Path]? = nil,
        networkMode: NetworkMode? = nil
    ) throws -> VMVirtualizationServiceContext {
        // This is a diagnostic log to track actual file paths on disk for debugging
        try validateDiskState()

        // Use provided networkMode, falling back to config value
        let effectiveNetworkMode = networkMode ?? vmDirContext.config.networkMode

        return VMVirtualizationServiceContext(
            cpuCount: cpuCount,
            memorySize: memorySize,
            display: display,
            sharedDirectories: sharedDirectories,
            mount: mount,
            hardwareModel: vmDirContext.config.hardwareModel,
            machineIdentifier: vmDirContext.config.machineIdentifier,
            macAddress: vmDirContext.config.macAddress!,
            diskPath: vmDirContext.diskPath,
            nvramPath: vmDirContext.nvramPath,
            recoveryMode: recoveryMode,
            usbMassStoragePaths: usbMassStoragePaths,
            networkMode: effectiveNetworkMode
        )
    }

    /// Validates the disk state to help diagnose storage attachment issues
    private func validateDiskState() throws {
        // Check disk image state
        let diskPath = vmDirContext.diskPath.path
        let diskExists = FileManager.default.fileExists(atPath: diskPath)
        var diskSize: UInt64 = 0
        var diskPermissions = ""

        if diskExists {
            if let attrs = try? FileManager.default.attributesOfItem(atPath: diskPath) {
                diskSize = attrs[.size] as? UInt64 ?? 0
                let posixPerms = attrs[.posixPermissions] as? Int ?? 0
                diskPermissions = String(format: "%o", posixPerms)
            }
        }

        // Check disk container directory permissions
        let diskDir = (diskPath as NSString).deletingLastPathComponent
        let dirPerms =
            try? FileManager.default.attributesOfItem(atPath: diskDir)[.posixPermissions] as? Int
            ?? 0
        let dirPermsString = dirPerms != nil ? String(format: "%o", dirPerms!) : "unknown"

        // Log detailed diagnostics
        Logger.info(
            "Validating VM disk state",
            metadata: [
                "diskPath": diskPath,
                "diskExists": "\(diskExists)",
                "diskSize":
                    "\(ByteCountFormatter.string(fromByteCount: Int64(diskSize), countStyle: .file))",
                "diskPermissions": diskPermissions,
                "dirPermissions": dirPermsString,
                "locationName": vmDirContext.storage ?? "home",
            ])

        if !diskExists {
            Logger.error("VM disk image does not exist", metadata: ["diskPath": diskPath])
        } else if diskSize == 0 {
            Logger.error("VM disk image exists but has zero size", metadata: ["diskPath": diskPath])
        }
    }

    func setup(
        ipswPath: String,
        cpuCount: Int,
        memorySize: UInt64,
        diskSize: UInt64,
        display: String
    ) async throws {
        fatalError("Must be implemented by subclass")
    }

    // MARK: - Finalization

    /// Post-installation step to move the VM directory to the home directory
    func finalize(to name: String, home: Home, storage: String? = nil) throws {
        let vmDir = try home.getVMDirectory(name, storage: storage)
        try FileManager.default.moveItem(at: vmDirContext.dir.dir.url, to: vmDir.dir.url)
    }

    // Method to run VM with additional USB mass storage devices
    func runWithUSBStorage(
        noDisplay: Bool, sharedDirectories: [SharedDirectory], mount: Path?, vncPort: Int = 0,
        recoveryMode: Bool = false, usbImagePaths: [Path]
    ) async throws {
        guard vmDirContext.initialized else {
            throw VMError.notInitialized(vmDirContext.name)
        }

        guard let cpuCount = vmDirContext.config.cpuCount,
            let memorySize = vmDirContext.config.memorySize
        else {
            throw VMError.notInitialized(vmDirContext.name)
        }

        // Try to acquire lock on config file
        let fileHandle = try FileHandle(forWritingTo: vmDirContext.dir.configPath.url)
        guard flock(fileHandle.fileDescriptor, LOCK_EX | LOCK_NB) == 0 else {
            try? fileHandle.close()
            throw VMError.alreadyRunning(vmDirContext.name)
        }

        Logger.info(
            "Running VM with USB storage devices",
            metadata: [
                "cpuCount": "\(cpuCount)",
                "memorySize": "\(memorySize)",
                "diskSize": "\(vmDirContext.config.diskSize ?? 0)",
                "usbImageCount": "\(usbImagePaths.count)",
                "recoveryMode": "\(recoveryMode)",
            ])

        // Create and configure the VM
        do {
            let config = try createVMVirtualizationServiceContext(
                cpuCount: cpuCount,
                memorySize: memorySize,
                display: vmDirContext.config.display.string,
                sharedDirectories: sharedDirectories,
                mount: mount,
                recoveryMode: recoveryMode,
                usbMassStoragePaths: usbImagePaths
            )
            virtualizationService = try virtualizationServiceFactory(config)

            let vncInfo = try await setupSession(
                port: vncPort, sharedDirectories: sharedDirectories)
            Logger.info("VNC info", metadata: ["vncInfo": vncInfo])

            // Start the VM
            guard let service = virtualizationService else {
                throw VMError.internalError("Virtualization service not initialized")
            }
            try await service.start()

            if !noDisplay {
                await waitForVisibleFramebufferBeforeOpeningClient()
                Logger.info("Starting VNC session", metadata: ["name": vmDirContext.name])
                try await vncService.openClient(url: vncInfo)
            }

            while true {
                try await Task.sleep(nanoseconds: UInt64(1e9))
            }
        } catch {
            Logger.error(
                "Failed to create/start VM with USB storage",
                metadata: [
                    "error": "\(error)",
                    "errorType": "\(type(of: error))",
                ])
            virtualizationService = nil
            vncService.stop()
            // Release lock
            flock(fileHandle.fileDescriptor, LOCK_UN)
            try? fileHandle.close()
            throw error
        }
    }
}
