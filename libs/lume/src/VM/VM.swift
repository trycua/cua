import Foundation

// MARK: - Support Types

/// Base context for virtual machine directory and configuration
struct VMDirContext {
    let dir: VMDirectory
    var config: VMConfig
    let home: Home

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
    private let vncService: VNCService
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

    // MARK: - VM State Management

    private var isRunning: Bool {
        // First check if we have an IP address
        guard let ipAddress = DHCPLeaseParser.getIPAddress(forMAC: vmDirContext.config.macAddress!)
        else {
            return false
        }

        // Then check if it's reachable
        return NetworkUtils.isReachable(ipAddress: ipAddress)
    }

    var details: VMDetails {
        let isRunning: Bool = self.isRunning
        let vncUrl = isRunning ? getVNCUrl() : nil

        return VMDetails(
            name: vmDirContext.name,
            os: getOSType(),
            cpuCount: vmDirContext.config.cpuCount ?? 0,
            memorySize: vmDirContext.config.memorySize ?? 0,
            diskSize: try! getDiskSize(),
            display: vmDirContext.config.display.string,
            status: isRunning ? "running" : "stopped",
            vncUrl: vncUrl,
            ipAddress: isRunning
                ? DHCPLeaseParser.getIPAddress(forMAC: vmDirContext.config.macAddress!) : nil
        )
    }

    // MARK: - VM Lifecycle Management

    func run(
        noDisplay: Bool, sharedDirectories: [SharedDirectory], mount: Path?, vncPort: Int = 0,
        recoveryMode: Bool = false
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
            "Running VM with configuration",
            metadata: [
                "cpuCount": "\(cpuCount)",
                "memorySize": "\(memorySize)",
                "diskSize": "\(vmDirContext.config.diskSize ?? 0)",
                "sharedDirectories": sharedDirectories.map(
                    { $0.string }
                ).joined(separator: ", "),
                "vncPort": "\(vncPort)",
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
                recoveryMode: recoveryMode
            )
            virtualizationService = try virtualizationServiceFactory(config)

            let vncInfo = try await setupVNC(noDisplay: noDisplay, port: vncPort)
            Logger.info("VNC info", metadata: ["vncInfo": vncInfo])

            // Start the VM
            guard let service = virtualizationService else {
                throw VMError.internalError("Virtualization service not initialized")
            }
            try await service.start()

            while true {
                try await Task.sleep(nanoseconds: UInt64(1e9))
            }
        } catch {
            virtualizationService = nil
            vncService.stop()
            // Release lock
            flock(fileHandle.fileDescriptor, LOCK_UN)
            try? fileHandle.close()
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
                try await service.stop()
                virtualizationService = nil
                vncService.stop()
                Logger.info(
                    "VM stopped successfully via virtualization service",
                    metadata: ["name": vmDirContext.name])
                return
            } catch let error {
                Logger.error(
                    "Failed to stop VM via virtualization service, falling back to process termination",
                    metadata: [
                        "name": vmDirContext.name,
                        "error": "\(error)",
                    ])
                // Fall through to process termination
            }
        }

        // Try to open config file to get file descriptor - note that this matches with the serve process - so this is only for the command line
        let fileHandle = try? FileHandle(forReadingFrom: vmDirContext.dir.configPath.url)
        guard let fileHandle = fileHandle else {
            Logger.error(
                "Failed to open config file - VM not running", metadata: ["name": vmDirContext.name]
            )
            throw VMError.notRunning(vmDirContext.name)
        }

        // Get the PID of the process holding the lock using lsof command
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
            Logger.error(
                "Failed to find VM process - VM not running", metadata: ["name": vmDirContext.name])
            throw VMError.notRunning(vmDirContext.name)
        }

        // First try graceful shutdown with SIGINT
        if kill(pid, SIGINT) == 0 {
            Logger.info(
                "Sent SIGINT to VM process", metadata: ["name": vmDirContext.name, "pid": "\(pid)"])
        }

        // Wait for process to stop with timeout
        var attempts = 0
        while attempts < 10 {
            try await Task.sleep(nanoseconds: 1_000_000_000)

            // Check if process still exists
            if kill(pid, 0) != 0 {
                // Process is gone, do final cleanup
                virtualizationService = nil
                vncService.stop()
                try? fileHandle.close()

                Logger.info(
                    "VM stopped successfully via process termination",
                    metadata: ["name": vmDirContext.name])
                return
            }
            attempts += 1
        }

        // If graceful shutdown failed, force kill the process
        Logger.info(
            "Graceful shutdown failed, forcing termination", metadata: ["name": vmDirContext.name])
        if kill(pid, SIGKILL) == 0 {
            // Wait a moment for the process to be fully killed
            try await Task.sleep(nanoseconds: 2_000_000_000)

            // Do final cleanup
            virtualizationService = nil
            vncService.stop()
            try? fileHandle.close()

            Logger.info("VM forcefully stopped", metadata: ["name": vmDirContext.name])
            return
        }

        // If we get here, something went very wrong
        try? fileHandle.close()
        Logger.error("Failed to stop VM", metadata: ["name": vmDirContext.name, "pid": "\(pid)"])
        throw VMError.internalError("Failed to stop VM process")
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

    private func setupVNC(noDisplay: Bool, port: Int = 0) async throws -> String {
        guard let service = virtualizationService else {
            throw VMError.internalError("Virtualization service not initialized")
        }

        try await vncService.start(port: port, virtualMachine: service.getVirtualMachine())

        guard let url = vncService.url else {
            throw VMError.vncNotConfigured
        }

        if !noDisplay {
            Logger.info("Starting VNC session")
            try await vncService.openClient(url: url)
        }

        return url
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
        recoveryMode: Bool = false
    ) throws -> VMVirtualizationServiceContext {
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
            recoveryMode: recoveryMode
        )
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
    func finalize(to name: String, home: Home, locationName: String? = nil) throws {
        let vmDir = try home.getVMDirectory(name, locationName: locationName)
        try FileManager.default.moveItem(at: vmDirContext.dir.dir.url, to: vmDir.dir.url)
    }
}
