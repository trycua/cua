import ArgumentParser
import Foundation
import Virtualization

// MARK: - Shared VM Manager

@MainActor
final class SharedVM {
    static let shared: SharedVM = SharedVM()
    private var runningVMs: [String: VM] = [:]

    private init() {}

    func getVM(name: String) -> VM? {
        return runningVMs[name]
    }

    func setVM(name: String, vm: VM) {
        runningVMs[name] = vm
    }

    func removeVM(name: String) {
        runningVMs.removeValue(forKey: name)
    }
}

/// Entrypoint for Commands and API server
final class LumeController {
    // MARK: - Properties

    let home: Home
    private let imageLoaderFactory: ImageLoaderFactory
    private let vmFactory: VMFactory

    // MARK: - Initialization

    init(
        home: Home = Home(),
        imageLoaderFactory: ImageLoaderFactory = DefaultImageLoaderFactory(),
        vmFactory: VMFactory = DefaultVMFactory()
    ) {
        self.home = home
        self.imageLoaderFactory = imageLoaderFactory
        self.vmFactory = vmFactory
    }

    // MARK: - Public VM Management Methods

    /// Lists all virtual machines in the system
    /// Uses a lightweight path that reads config directly without instantiating full VM objects
    @MainActor
    public func list(storage: String? = nil) throws -> [VMDetails] {
        do {
            if let storage = storage {
                // If storage is specified, only return VMs from that location
                if storage.contains("/") || storage.contains("\\") {
                    // Direct path - check if it exists
                    if !FileManager.default.fileExists(atPath: storage) {
                        // Return empty array if the path doesn't exist
                        return []
                    }

                    // Try to get all VMs from the specified path
                    let directoryURL = URL(fileURLWithPath: storage)
                    let contents = try FileManager.default.contentsOfDirectory(
                        at: directoryURL,
                        includingPropertiesForKeys: [.isDirectoryKey],
                        options: .skipsHiddenFiles
                    )

                    let statuses = contents.compactMap { subdir -> VMDetails? in
                        guard let isDirectory = try? subdir.resourceValues(forKeys: [.isDirectoryKey]).isDirectory,
                              isDirectory else {
                            return nil
                        }

                        let vmName = subdir.lastPathComponent
                        guard let vmDir = try? home.getVMDirectoryFromPath(vmName, storagePath: storage),
                              vmDir.initialized() else {
                            return nil
                        }

                        return getVMDetailsLightweight(vmDir: vmDir, locationName: storage)
                    }
                    return statuses.sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
                } else {
                    // Named storage
                    let vmsWithLoc = try home.getAllVMDirectories()
                    let statuses = vmsWithLoc.compactMap { vmWithLoc -> VMDetails? in
                        // Only include VMs from the specified location
                        if vmWithLoc.locationName != storage {
                            return nil
                        }
                        return getVMDetailsLightweight(
                            vmDir: vmWithLoc.directory,
                            locationName: vmWithLoc.locationName
                        )
                    }
                    return statuses.sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
                }
            } else {
                // No storage filter - get all VMs
                let vmsWithLoc = try home.getAllVMDirectories()
                let statuses = vmsWithLoc.compactMap { vmWithLoc -> VMDetails? in
                    return getVMDetailsLightweight(
                        vmDir: vmWithLoc.directory,
                        locationName: vmWithLoc.locationName
                    )
                }
                return statuses.sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
            }
        } catch {
            Logger.error("Failed to list VMs", metadata: ["error": error.localizedDescription])
            throw error
        }
    }

    /// Parses the VNC port from a VNC URL
    /// - Parameter url: VNC URL like "vnc://:password@127.0.0.1:62295"
    /// - Returns: The port number if successfully parsed, nil otherwise
    private func parseVNCPort(from url: String) -> UInt16? {
        // URL format: vnc://:password@host:port
        guard let urlComponents = URLComponents(string: url.replacingOccurrences(of: "vnc://", with: "http://")),
              let port = urlComponents.port else {
            return nil
        }
        return UInt16(port)
    }

    /// Get VM details using lightweight path (no VM object instantiation)
    /// Checks provisioning marker first, then SharedVM cache for running status
    @MainActor
    private func getVMDetailsLightweight(vmDir: VMDirectory, locationName: String) -> VMDetails? {
        let vmName = vmDir.name

        // Check provisioning marker FIRST - if present, VM may be in creation
        if let marker = vmDir.loadProvisioningMarker() {
            // Check if VM is actually complete (has all required files)
            // If complete, the marker is stale and should be auto-cleaned
            let hasRequiredFiles = vmDir.diskPath.exists() && vmDir.nvramPath.exists()
            
            if hasRequiredFiles {
                // VM is complete but marker wasn't cleaned up (e.g., after unattended setup)
                // Auto-cleanup the stale marker
                vmDir.clearProvisioningMarker()
                Logger.info("Auto-cleaned stale provisioning marker for complete VM", metadata: ["name": vmName])
                // Fall through to normal status check below
            } else {
                // VM is still being provisioned
                let status = marker.isStale() ? "provisioning (stale)" : "provisioning"
                if marker.isStale() {
                    Logger.info("VM provisioning may be stuck", metadata: [
                        "name": vmName,
                        "operation": marker.operation,
                        "hint": "If creation was interrupted, delete with: lume delete \(vmName)"
                    ])
                }
                return vmDir.getDetails(
                    locationName: locationName,
                    status: status,
                    provisioningOperation: marker.operation,
                    vncUrl: nil,
                    ipAddress: nil,
                    sshAvailable: nil
                )
            }
        }

        // Check if VM is running via SharedVM cache (same-process fast path)
        let runningVM = SharedVM.shared.getVM(name: vmName)
        var isRunning = runningVM != nil

        // Get VNC URL and IP address only if running
        var vncUrl: String? = nil
        var ipAddress: String? = nil
        var sshAvailable: Bool? = nil

        // If not in cache, check if session file exists (cross-process fallback)
        // Session files are created when VM starts and deleted when VM stops
        // Validate that the VNC port is actually in use to detect stale sessions
        if !isRunning {
            if let session = try? vmDir.loadSession() {
                // Parse VNC port from URL like "vnc://:password@127.0.0.1:62295"
                if let port = parseVNCPort(from: session.url),
                   NetworkUtils.isLocalPortInUse(port: port) {
                    isRunning = true
                    vncUrl = session.url
                } else {
                    // Stale session file - VNC port not in use, clean it up
                    vmDir.clearSession()
                    Logger.info("Cleaned up stale session file", metadata: ["name": vmName])
                }
            }
        }

        if isRunning {
            // Try to get VNC URL from session file (if not already loaded)
            if vncUrl == nil {
                vncUrl = try? vmDir.loadSession().url
            }

            // Try to get IP address from DHCP lease if we have MAC address
            if let config = try? vmDir.loadConfig(),
               let macAddress = config.macAddress {
                ipAddress = DHCPLeaseParser.getIPAddress(forMAC: macAddress)

                // Check if SSH is available
                if let ip = ipAddress {
                    sshAvailable = NetworkUtils.isSSHAvailable(ipAddress: ip)
                }
            }
        }

        return vmDir.getDetails(
            locationName: locationName,
            status: isRunning ? "running" : "stopped",
            provisioningOperation: nil,
            vncUrl: vncUrl,
            ipAddress: ipAddress,
            sshAvailable: sshAvailable
        )
    }


    /// Validates that a VM is not currently being provisioned
    /// - Parameters:
    ///   - vmDir: The VM directory to check
    ///   - name: The VM name (for error message)
    /// - Throws: VMError.stillProvisioning if the VM has a provisioning marker
    private func validateNotProvisioning(_ vmDir: VMDirectory, name: String) throws {
        if vmDir.loadProvisioningMarker() != nil {
            throw VMError.stillProvisioning(name)
        }
    }

    @MainActor
    public func clone(
        name: String, 
        newName: String, 
        sourceLocation: String? = nil, 
        destLocation: String? = nil,
        compact: Bool = false,
        expandBy: String? = nil
    ) throws {
        let normalizedName = normalizeVMName(name: name)
        let normalizedNewName = normalizeVMName(name: newName)
        Logger.info(
            "Cloning VM",
            metadata: [
                "source": normalizedName,
                "destination": normalizedNewName,
                "sourceLocation": sourceLocation ?? "default",
                "destLocation": destLocation ?? "default",
            ])

        do {
            // Validate source VM exists
            let actualSourceLocation = try self.validateVMExists(normalizedName, storage: sourceLocation)

            // Check if source VM is still being provisioned
            let sourceVmDir = try home.getVMDirectory(normalizedName, storage: actualSourceLocation)
            try validateNotProvisioning(sourceVmDir, name: normalizedName)

            // Get the source VM and check if it's running
            let sourceVM = try get(name: normalizedName, storage: sourceLocation)
            if sourceVM.details.status == "running" {
                Logger.error("Cannot clone a running VM", metadata: ["source": normalizedName])
                throw VMError.alreadyRunning(normalizedName)
            }

            // Check if destination already exists
            do {
                let destDir = try home.getVMDirectory(normalizedNewName, storage: destLocation)
                if destDir.exists() {
                    Logger.error(
                        "Destination VM already exists",
                        metadata: ["destination": normalizedNewName])
                    throw HomeError.directoryAlreadyExists(path: destDir.dir.path)
                }
            } catch VMLocationError.locationNotFound {
                // Location not found is okay, we'll create it
            } catch VMError.notFound {
                // VM not found is okay, we'll create it
            }

            // Copy the VM directory
            // If we need to modify the disk (compact or expand), we need to handle copying manually
            if compact || expandBy != nil {
                try home.copyVMDirectoryManual(
                    from: normalizedName,
                    to: normalizedNewName,
                    sourceLocation: sourceLocation,
                    destLocation: destLocation,
                    compact: compact
                )
                
                // Handle expansion if requested
                if let expandAmountStr = expandBy {
                    let expandAmount = try parseSize(expandAmountStr)
                    let destVM = try get(name: normalizedNewName, storage: destLocation)
                    let currentSize = try destVM.getDiskSize()
                    let newSize = currentSize.total + expandAmount
                    
                    Logger.info("Expanding disk", metadata: [
                        "current": "\(currentSize.total)",
                        "add": "\(expandAmount)",
                        "new": "\(newSize)"
                    ])
                    
                    try destVM.resizeDisk(newSize)
                }
            } else {
                // Standard copy
                try home.copyVMDirectory(
                    from: normalizedName,
                    to: normalizedNewName,
                    sourceLocation: sourceLocation,
                    destLocation: destLocation
                )
            }

            // Update MAC address in the cloned VM to ensure uniqueness
            let clonedVM = try get(name: normalizedNewName, storage: destLocation)
            try clonedVM.setMacAddress(VZMACAddress.randomLocallyAdministered().string)

            // Update MAC Identifier in the cloned VM to ensure uniqueness
            try clonedVM.setMachineIdentifier(
                DarwinVirtualizationService.generateMachineIdentifier())

            Logger.info(
                "VM cloned successfully",
                metadata: ["source": normalizedName, "destination": normalizedNewName])
        } catch {
            Logger.error("Failed to clone VM", metadata: ["error": error.localizedDescription])
            throw error
        }
    }

    @MainActor
    public func get(name: String, storage: String? = nil) throws -> VM {
        let normalizedName = normalizeVMName(name: name)
        do {
            let vm: VM
            if let storagePath = storage, storagePath.contains("/") || storagePath.contains("\\") {
                // Storage is a direct path
                let vmDir = try home.getVMDirectoryFromPath(normalizedName, storagePath: storagePath)
                guard vmDir.initialized() else {
                    // Throw a specific error if the directory exists but isn't a valid VM
                    if vmDir.exists() {
                        throw VMError.notInitialized(normalizedName)
                    } else {
                        throw VMError.notFound(normalizedName)
                    }
                }
                // Pass the path as the storage context
                vm = try self.loadVM(vmDir: vmDir, storage: storagePath)
            } else {
                // Storage is nil or a named location
                let actualLocation = try self.validateVMExists(
                    normalizedName, storage: storage)

                let vmDir = try home.getVMDirectory(normalizedName, storage: actualLocation)
                // loadVM will re-check initialized, but good practice to keep validateVMExists result.
                vm = try self.loadVM(vmDir: vmDir, storage: actualLocation)
            }
            return vm
        } catch {
            Logger.error(
                "Failed to get VM",
                metadata: [
                    "vmName": normalizedName, "storage": storage ?? "home",
                    "error": error.localizedDescription,
                ])
            // Re-throw the original error to preserve its type
            throw error
        }
    }

    /// Gets VM details using the lightweight path (includes provisioning status)
    /// Use this instead of get().details when you need accurate status including provisioning state
    @MainActor
    public func getDetails(name: String, storage: String? = nil) throws -> VMDetails {
        let normalizedName = normalizeVMName(name: name)
        do {
            let vmDir: VMDirectory
            let locationName: String

            if let storagePath = storage, storagePath.contains("/") || storagePath.contains("\\") {
                // Storage is a direct path
                vmDir = try home.getVMDirectoryFromPath(normalizedName, storagePath: storagePath)
                guard vmDir.initialized() else {
                    if vmDir.exists() {
                        throw VMError.notInitialized(normalizedName)
                    } else {
                        throw VMError.notFound(normalizedName)
                    }
                }
                locationName = storagePath
            } else {
                // Storage is nil or a named location - find the VM
                let actualLocation = try self.validateVMExists(normalizedName, storage: storage)
                vmDir = try home.getVMDirectory(normalizedName, storage: actualLocation)
                locationName = actualLocation ?? "home"
            }

            // Use the lightweight path that includes provisioning status
            guard let details = getVMDetailsLightweight(vmDir: vmDir, locationName: locationName) else {
                throw VMError.notFound(normalizedName)
            }
            return details
        } catch {
            Logger.error(
                "Failed to get VM details",
                metadata: [
                    "vmName": normalizedName, "storage": storage ?? "home",
                    "error": error.localizedDescription,
                ])
            throw error
        }
    }

    @MainActor
    public func create(
        name: String,
        os: String,
        diskSize: UInt64,
        cpuCount: Int,
        memorySize: UInt64,
        display: String,
        ipsw: String?,
        storage: String? = nil,
        unattendedConfig: UnattendedConfig? = nil,
        debug: Bool = false,
        debugDir: String? = nil,
        noDisplay: Bool = true,
        vncPort: Int = 0,
        networkMode: NetworkMode = .nat
    ) async throws {
        Logger.info(
            "Creating VM",
            metadata: [
                "name": name,
                "os": os,
                "location": storage ?? "home",
                "disk_size": "\(diskSize / 1024 / 1024)MB",
                "cpu_count": "\(cpuCount)",
                "memory_size": "\(memorySize / 1024 / 1024)MB",
                "display": display,
                "ipsw": ipsw ?? "none",
                "unattended": unattendedConfig != nil ? "yes" : "no",
                "debug": "\(debug)",
                "noDisplay": "\(noDisplay)",
            ])

        // Validate parameters upfront
        try validateCreateParameters(name: name, os: os, ipsw: ipsw, storage: storage)

        // Create target VM directory early with provisioning marker
        // so VM appears in list immediately with "provisioning" status
        let vmDir = try home.getVMDirectory(name, storage: storage)

        do {
            try FileManager.default.createDirectory(
                atPath: vmDir.dir.path,
                withIntermediateDirectories: true
            )

            // Create minimal config so VM shows up in list
            let config = try VMConfig(
                os: os,
                cpuCount: cpuCount,
                memorySize: memorySize,
                diskSize: diskSize,
                display: display,
                networkMode: networkMode
            )
            try vmDir.saveConfig(config)

            // Write provisioning marker
            try vmDir.saveProvisioningMarker(ProvisioningMarker(operation: "ipsw_install"))
            Logger.info("Provisioning marker created", metadata: ["name": name])
        } catch {
            // Clean up if we fail to set up provisioning marker
            try? vmDir.delete()
            Logger.error("Failed to create VM", metadata: ["error": error.localizedDescription])
            throw error
        }

        do {
            // Use createInternal which handles all the actual work
            try await createInternal(
                name: name,
                os: os,
                diskSize: diskSize,
                cpuCount: cpuCount,
                memorySize: memorySize,
                display: display,
                ipsw: ipsw,
                storage: storage,
                unattendedConfig: unattendedConfig,
                debug: debug,
                debugDir: debugDir,
                noDisplay: noDisplay,
                vncPort: vncPort,
                vmDir: vmDir,
                networkMode: networkMode
            )

            // Clear provisioning marker on success
            vmDir.clearProvisioningMarker()
            Logger.info("Provisioning marker cleared", metadata: ["name": name])

        } catch {
            // Clear provisioning marker on failure (vmDir may have been deleted by createInternal)
            vmDir.clearProvisioningMarker()
            Logger.error("Failed to create VM", metadata: ["error": error.localizedDescription])
            throw error
        }
    }

    /// Creates a VM asynchronously, returning immediately while the VM is being provisioned.
    /// The VM will appear with status "provisioning" in `lume ls` until creation completes.
    /// Poll VM status to check progress.
    ///
    /// - Parameters:
    ///   - name: Name for the new VM
    ///   - os: Operating system type ("macos" or "linux")
    ///   - diskSize: Disk size in bytes
    ///   - cpuCount: Number of CPU cores
    ///   - memorySize: Memory size in bytes
    ///   - display: Display resolution string (e.g., "1920x1080")
    ///   - ipsw: Path to IPSW file or "latest" for macOS
    ///   - storage: Optional storage location name or path
    ///   - unattendedConfig: Optional unattended setup configuration
    @MainActor
    public func createAsync(
        name: String,
        os: String,
        diskSize: UInt64,
        cpuCount: Int,
        memorySize: UInt64,
        display: String,
        ipsw: String?,
        storage: String? = nil,
        unattendedConfig: UnattendedConfig? = nil,
        networkMode: NetworkMode = .nat
    ) throws {
        Logger.info(
            "Starting async VM creation",
            metadata: [
                "name": name,
                "os": os,
                "location": storage ?? "home",
                "unattended": unattendedConfig != nil ? "yes" : "no",
            ])

        // Validate parameters upfront (this checks VM doesn't already exist)
        try validateCreateParameters(name: name, os: os, ipsw: ipsw, storage: storage)

        // Create VM directory and provisioning marker BEFORE spawning background task
        // so VM appears in list immediately with "provisioning" status
        let vmDir = try home.getVMDirectory(name, storage: storage)

        do {
            try FileManager.default.createDirectory(
                atPath: vmDir.dir.path,
                withIntermediateDirectories: true
            )

            // Create minimal config so VM shows up in list
            let config = try VMConfig(
                os: os,
                cpuCount: cpuCount,
                memorySize: memorySize,
                diskSize: diskSize,
                display: display,
                networkMode: networkMode
            )
            try vmDir.saveConfig(config)

            // Write provisioning marker
            try vmDir.saveProvisioningMarker(ProvisioningMarker(operation: "ipsw_install"))
            Logger.info("Provisioning marker created", metadata: ["name": name])
        } catch {
            // Clean up if we fail to set up provisioning marker
            try? vmDir.delete()
            Logger.error("Failed to create VM", metadata: ["error": error.localizedDescription])
            throw error
        }

        Logger.info("Spawning background task for VM creation", metadata: ["name": name])

        // All parameters passed to Task are value types (Sendable)
        // The Task will create its own LumeController instance
        Task.detached { @MainActor @Sendable in
            // Create a new controller for the background task
            let controller = LumeController()

            do {
                // Run the internal create which does all the work
                // (skips validation since we already validated)
                try await controller.createInternal(
                    name: name,
                    os: os,
                    diskSize: diskSize,
                    cpuCount: cpuCount,
                    memorySize: memorySize,
                    display: display,
                    ipsw: ipsw,
                    storage: storage,
                    unattendedConfig: unattendedConfig,
                    vmDir: vmDir,
                    networkMode: networkMode
                )

                // Clear marker on success
                vmDir.clearProvisioningMarker()
                Logger.info("Async VM creation completed successfully", metadata: ["name": name])

            } catch {
                // Clear marker and cleanup on failure
                vmDir.clearProvisioningMarker()
                do {
                    try vmDir.delete()
                } catch let cleanupError {
                    Logger.error("Failed to clean up VM directory after async creation failure",
                               metadata: ["error": cleanupError.localizedDescription])
                }
                Logger.error("Async VM creation failed",
                            metadata: ["name": name, "error": error.localizedDescription])
            }
        }
    }

    /// Internal create method that skips validation (used by createAsync)
    @MainActor
    private func createInternal(
        name: String,
        os: String,
        diskSize: UInt64,
        cpuCount: Int,
        memorySize: UInt64,
        display: String,
        ipsw: String?,
        storage: String? = nil,
        unattendedConfig: UnattendedConfig? = nil,
        debug: Bool = false,
        debugDir: String? = nil,
        noDisplay: Bool = true,
        vncPort: Int = 0,
        vmDir: VMDirectory? = nil,
        networkMode: NetworkMode = .nat
    ) async throws {
        Logger.info(
            "Creating VM (internal)",
            metadata: [
                "name": name,
                "os": os,
                "location": storage ?? "home",
            ])

        let vm = try await createTempVMConfig(
            os: os,
            cpuCount: cpuCount,
            memorySize: memorySize,
            diskSize: diskSize,
            display: display,
            networkMode: networkMode
        )

        // Track the temp directory for cleanup on failure
        let tempVMDir = vm.vmDirContext.dir

        do {
            try await vm.setup(
                ipswPath: ipsw ?? "none",
                cpuCount: cpuCount,
                memorySize: memorySize,
                diskSize: diskSize,
                display: display
            )

            // If vmDir was pre-created (async flow), we need to handle finalization differently
            if let existingVmDir = vmDir {
                // Delete the pre-created directory (with just config and marker)
                // and let finalize create it fresh with all the VM files
                try existingVmDir.delete()
            }

            try vm.finalize(to: name, home: home, storage: storage)
        } catch {
            // Clean up the temporary VM directory on setup/finalize failure
            Logger.info("Cleaning up temporary VM directory after failed creation",
                       metadata: ["path": tempVMDir.dir.path])
            do {
                try tempVMDir.delete()
            } catch let cleanupError {
                Logger.error("Failed to clean up temporary VM directory",
                           metadata: ["error": cleanupError.localizedDescription])
            }
            throw error
        }

        Logger.info("VM created successfully", metadata: ["name": name])

        // Run unattended setup if config is provided
        if let config = unattendedConfig, os.lowercased() == "macos" {
            // Note: We don't write a provisioning marker for unattended setup.
            // The VM has disk + nvram at this point, so it's "running" during
            // the setup automation, not "provisioning".

            // Wait for the installation VZVirtualMachine to fully release auxiliary storage locks.
            Logger.info("Waiting for installation resources to be released before unattended setup")
            try await Task.sleep(nanoseconds: 3_000_000_000)  // 3 seconds
            Logger.info("Starting unattended Setup Assistant automation", metadata: ["name": name])

            // Load the finalized VM
            let finalVM = try get(name: name, storage: storage)

            // Run the unattended installer
            let installer = UnattendedInstaller()
            do {
                try await installer.install(
                    vm: finalVM,
                    config: config,
                    vncPort: vncPort,
                    noDisplay: noDisplay,
                    debug: debug,
                    debugDir: debugDir
                )
            } catch {
                // Clean up the finalized VM directory on unattended setup failure
                Logger.info("Cleaning up VM after failed unattended setup",
                           metadata: ["name": name])
                do {
                    let vmDirToDelete = try home.getVMDirectory(name, storage: storage)
                    try vmDirToDelete.delete()
                } catch let cleanupError {
                    Logger.error("Failed to clean up VM after unattended setup failure",
                               metadata: ["error": cleanupError.localizedDescription])
                }
                throw error
            }

            Logger.info("Unattended setup completed", metadata: ["name": name])
        }
    }


    /// Run unattended Setup Assistant automation on an existing macOS VM
    @MainActor
    public func setup(
        name: String,
        config: UnattendedConfig,
        storage: String? = nil,
        vncPort: Int = 0,
        noDisplay: Bool = false,
        debug: Bool = false,
        debugDir: String? = nil
    ) async throws {
        let normalizedName = normalizeVMName(name: name)
        Logger.info(
            "Running unattended setup",
            metadata: [
                "name": normalizedName,
                "storage": storage ?? "home",
                "bootWait": "\(config.bootWait)s",
                "commands": "\(config.bootCommands.count)",
                "debug": "\(debug)",
                "debugDir": debugDir ?? "default"
            ])

        do {
            // Get the VM
            let vm = try get(name: normalizedName, storage: storage)

            // Check if it's a macOS VM
            guard vm.config.os.lowercased() == "macos" else {
                throw VMError.unsupportedOS("Unattended setup is only supported for macOS VMs, got: \(vm.config.os)")
            }

            // Run the unattended installer
            let installer = UnattendedInstaller()
            try await installer.install(vm: vm, config: config, vncPort: vncPort, noDisplay: noDisplay, debug: debug, debugDir: debugDir)

            Logger.info("Unattended setup completed", metadata: ["name": normalizedName])
        } catch {
            Logger.error("Failed to run unattended setup", metadata: ["error": error.localizedDescription])
            throw error
        }
    }

    @MainActor
    public func delete(name: String, storage: String? = nil) async throws {
        let normalizedName = normalizeVMName(name: name)
        Logger.info(
            "Deleting VM",
            metadata: [
                "name": normalizedName,
                "location": storage ?? "home",
            ])

        do {
            let vmDir: VMDirectory
            
            // Check if storage is a direct path
            if let storagePath = storage, storagePath.contains("/") || storagePath.contains("\\") {
                // Storage is a direct path
                vmDir = try home.getVMDirectoryFromPath(normalizedName, storagePath: storagePath)
                guard vmDir.initialized() else {
                    // Throw a specific error if the directory exists but isn't a valid VM
                    if vmDir.exists() {
                        throw VMError.notInitialized(normalizedName)
                    } else {
                        throw VMError.notFound(normalizedName)
                    }
                }
            } else {
                // Storage is nil or a named location
                let actualLocation = try self.validateVMExists(normalizedName, storage: storage)
                vmDir = try home.getVMDirectory(normalizedName, storage: actualLocation)
            }
            
            // Stop VM if it's running
            if SharedVM.shared.getVM(name: normalizedName) != nil {
                try await stopVM(name: normalizedName)
            }
            
            try vmDir.delete()
            
            Logger.info("VM deleted successfully", metadata: ["name": normalizedName])
            
        } catch {
            Logger.error("Failed to delete VM", metadata: ["error": error.localizedDescription])
            throw error
        }
    }

    // MARK: - VM Operations

    @MainActor
    public func updateSettings(
        name: String,
        cpu: Int? = nil,
        memory: UInt64? = nil,
        diskSize: UInt64? = nil,
        display: String? = nil,
        storage: String? = nil
    ) throws {
        let normalizedName = normalizeVMName(name: name)
        Logger.info(
            "Updating VM settings",
            metadata: [
                "name": normalizedName,
                "location": storage ?? "home",
                "cpu": cpu.map { "\($0)" } ?? "unchanged",
                "memory": memory.map { "\($0 / 1024 / 1024)MB" } ?? "unchanged",
                "disk_size": diskSize.map { "\($0 / 1024 / 1024)MB" } ?? "unchanged",
                "display": display ?? "unchanged",
            ])
        do {
            // Find the actual location of the VM
            let actualLocation = try self.validateVMExists(
                normalizedName, storage: storage)

            let vm = try get(name: normalizedName, storage: actualLocation)

            // Apply settings in order
            if let cpu = cpu {
                try vm.setCpuCount(cpu)
            }
            if let memory = memory {
                try vm.setMemorySize(memory)
            }
            if let diskSize = diskSize {
                try vm.setDiskSize(diskSize)
            }
            if let display = display {
                try vm.setDisplay(display)
            }

            Logger.info("VM settings updated successfully", metadata: ["name": normalizedName])
        } catch {
            Logger.error(
                "Failed to update VM settings", metadata: ["error": error.localizedDescription])
            throw error
        }
    }

    @MainActor
    public func stopVM(name: String, storage: String? = nil) async throws {
        let normalizedName = normalizeVMName(name: name)
        Logger.info("Stopping VM", metadata: ["name": normalizedName])

        do {
            // Find the actual location of the VM
            let actualLocation = try self.validateVMExists(
                normalizedName, storage: storage)

            // Check if VM is still being provisioned
            let vmDir = try home.getVMDirectory(normalizedName, storage: actualLocation)
            try validateNotProvisioning(vmDir, name: normalizedName)

            // Try to get VM from cache first
            let vm: VM
            if let cachedVM = SharedVM.shared.getVM(name: normalizedName) {
                vm = cachedVM
            } else {
                vm = try get(name: normalizedName, storage: actualLocation)
            }

            try await vm.stop()
            // Remove VM from cache after stopping
            SharedVM.shared.removeVM(name: normalizedName)
            Logger.info("VM stopped successfully", metadata: ["name": normalizedName])
        } catch {
            // Clean up cache even if stop fails
            SharedVM.shared.removeVM(name: normalizedName)
            Logger.error("Failed to stop VM", metadata: ["error": error.localizedDescription])
            throw error
        }
    }

    @MainActor
    public func runVM(
        name: String,
        noDisplay: Bool = false,
        sharedDirectories: [SharedDirectory] = [],
        mount: Path? = nil,
        registry: String = "ghcr.io",
        organization: String = "trycua",
        vncPort: Int = 0,
        recoveryMode: Bool = false,
        storage: String? = nil,
        usbMassStoragePaths: [Path]? = nil,
        networkMode: NetworkMode? = nil,
        clipboard: Bool = false
    ) async throws {
        let normalizedName = normalizeVMName(name: name)
        Logger.info(
            "Running VM",
            metadata: [
                "name": normalizedName,
                "no_display": "\(noDisplay)",
                "shared_directories":
                    "\(sharedDirectories.map( { $0.string } ).joined(separator: ", "))",
                "mount": mount?.path ?? "none",
                "vnc_port": "\(vncPort)",
                "recovery_mode": "\(recoveryMode)",
                "storage_param": storage ?? "home", // Log the original param
                "usb_storage_devices": "\(usbMassStoragePaths?.count ?? 0)",
                "network_override": networkMode?.description ?? "vm-config",
            ])

        do {
            // Check if name is an image ref to auto-pull
            let components = normalizedName.split(separator: ":")
            if components.count == 2 { // Check if it looks like image:tag
                // Attempt to validate if VM exists first, suppressing the error
                // This avoids pulling if the VM already exists, even if name looks like an image ref
                let vmExists = (try? self.validateVMExists(normalizedName, storage: storage)) != nil
                if !vmExists {
                    Logger.info(
                        "VM not found, attempting to pull image based on name",
                        metadata: ["imageRef": normalizedName])
                    // Use the potentially new VM name derived from the image ref
                    let potentialVMName = String(components[0])
                    try await pullImage(
                        image: normalizedName, // Full image ref
                        name: potentialVMName, // Name derived from image
                        registry: registry,
                        organization: organization,
                        storage: storage
                    )
                    // Important: After pull, the effective name might have changed
                    // We proceed assuming the user wants to run the VM derived from image name
                    // normalizedName = potentialVMName // Re-assign normalizedName if pull logic creates it
                    // Note: Current pullImage doesn't return the final VM name, 
                    // so we assume it matches the name derived from the image.
                    // This might need refinement if pullImage behaviour changes.
                }
            }

            // Determine effective storage path or name AND get the VMDirectory
            let effectiveStorage: String?
            let vmDir: VMDirectory

            if let storagePath = storage, storagePath.contains("/") || storagePath.contains("\\") {
                // Storage is a direct path
                vmDir = try home.getVMDirectoryFromPath(normalizedName, storagePath: storagePath)
                guard vmDir.initialized() else {
                    if vmDir.exists() {
                        throw VMError.notInitialized(normalizedName)
                    } else {
                        throw VMError.notFound(normalizedName)
                    }
                }
                effectiveStorage = storagePath // Use the path string
                Logger.info("Using direct storage path", metadata: ["path": storagePath])
            } else {
                // Storage is nil or a named location - validate and get the actual name
                let actualLocationName = try validateVMExists(normalizedName, storage: storage)
                vmDir = try home.getVMDirectory(normalizedName, storage: actualLocationName) // Get VMDir for named location
                effectiveStorage = actualLocationName // Use the named location string
                Logger.info(
                    "Using named storage location",
                    metadata: [
                        "requested": storage ?? "home",
                        "actual": actualLocationName ?? "default",
                    ])
            }

            // Check if VM is still being provisioned
            try validateNotProvisioning(vmDir, name: normalizedName)

            // Validate parameters using the located VMDirectory
            try validateRunParameters(
                vmDir: vmDir, // Pass vmDir
                sharedDirectories: sharedDirectories,
                mount: mount,
                usbMassStoragePaths: usbMassStoragePaths
            )

            // Load the VM directly using the located VMDirectory and storage context
            let vm = try self.loadVM(vmDir: vmDir, storage: effectiveStorage)

            SharedVM.shared.setVM(name: normalizedName, vm: vm)
            try await vm.run(
                noDisplay: noDisplay,
                sharedDirectories: sharedDirectories,
                mount: mount,
                vncPort: vncPort,
                recoveryMode: recoveryMode,
                usbMassStoragePaths: usbMassStoragePaths,
                networkMode: networkMode,
                clipboard: clipboard)
            Logger.info("VM started successfully", metadata: ["name": normalizedName])
        } catch {
            SharedVM.shared.removeVM(name: normalizedName)
            Logger.error("Failed to run VM", metadata: ["error": error.localizedDescription])
            throw error
        }
    }

    // MARK: - Image Management

    @MainActor
    public func getLatestIPSWURL() async throws -> URL {
        Logger.info("Fetching latest supported IPSW URL")

        do {
            let imageLoader = DarwinImageLoader()
            let url = try await imageLoader.fetchLatestSupportedURL()
            Logger.info("Found latest IPSW URL", metadata: ["url": url.absoluteString])
            return url
        } catch {
            Logger.error(
                "Failed to fetch IPSW URL", metadata: ["error": error.localizedDescription])
            throw error
        }
    }

    @MainActor
    public func pullImage(
        image: String,
        name: String?,
        registry: String,
        organization: String,
        storage: String? = nil
    ) async throws {
        do {
            // Split the image to get name and tag
            let components = image.split(separator: ":")
            guard components.count == 2 else {
                throw ValidationError("Invalid image format. Expected format: name:tag")
            }

            let imageName = String(components[0])
            let tag = String(components[1])

            // Set default VM name if not provided
            let vmName = name ?? "\(imageName)_\(tag)"

            Logger.info(
                "Pulling image",
                metadata: [
                    "image": image,
                    "name": vmName,
                    "registry": registry,
                    "organization": organization,
                    "location": storage ?? "home",
                ])

            try self.validatePullParameters(
                image: image,
                name: vmName,
                registry: registry,
                organization: organization,
                storage: storage
            )

            let imageRegistry = try RegistryFactory.createRegistry(
                registry: registry, organization: organization)
            let _ = try await imageRegistry.pull(
                image: image,
                name: vmName,
                locationName: storage)

            Logger.info(
                "Setting new VM mac address",
                metadata: [
                    "vm_name": vmName,
                    "location": storage ?? "home",
                ])

            // Update MAC address in the cloned VM to ensure uniqueness
            let vm = try get(name: vmName, storage: storage)
            try vm.setMacAddress(VZMACAddress.randomLocallyAdministered().string)

            Logger.info(
                "Image pulled successfully",
                metadata: [
                    "image": image,
                    "name": vmName,
                    "registry": registry,
                    "organization": organization,
                    "location": storage ?? "home",
                ])
        } catch {
            Logger.error("Failed to pull image", metadata: ["error": error.localizedDescription])
            throw error
        }
    }

    @MainActor
    public func pushImage(
        name: String,
        imageName: String,
        tags: [String],
        registry: String,
        organization: String,
        storage: String? = nil,
        chunkSizeMb: Int = 512,
        verbose: Bool = false,
        dryRun: Bool = false,
        reassemble: Bool = false
    ) async throws {
        do {
            Logger.info(
                "Pushing VM to registry",
                metadata: [
                    "name": name,
                    "imageName": imageName,
                    "tags": "\(tags.joined(separator: ", "))",
                    "registry": registry,
                    "organization": organization,
                    "location": storage ?? "home",
                    "chunk_size": "\(chunkSizeMb)MB",
                    "dry_run": "\(dryRun)",
                    "reassemble": "\(reassemble)",
                ])

            try validatePushParameters(
                name: name,
                imageName: imageName,
                tags: tags,
                registry: registry,
                organization: organization
            )

            // Find the actual location of the VM
            let actualLocation = try self.validateVMExists(name, storage: storage)

            // Get the VM directory
            let vmDir = try home.getVMDirectory(name, storage: actualLocation)

            // Use configured registry to push the VM
            let imageRegistry = try RegistryFactory.createRegistry(
                registry: registry, organization: organization)

            try await imageRegistry.push(
                vmDirPath: vmDir.dir.path,
                imageName: imageName,
                tags: tags,
                chunkSizeMb: chunkSizeMb,
                verbose: verbose,
                dryRun: dryRun,
                reassemble: reassemble
            )

            Logger.info(
                "VM pushed successfully",
                metadata: [
                    "name": name,
                    "imageName": imageName,
                    "tags": "\(tags.joined(separator: ", "))",
                    "registry": registry,
                    "organization": organization,
                ])
        } catch {
            Logger.error("Failed to push VM", metadata: ["error": error.localizedDescription])
            throw error
        }
    }

    @MainActor
    public func pruneImages() async throws {
        Logger.info("Pruning cached images")

        do {
            // Use configured cache directory
            let cacheDir = (SettingsManager.shared.getCacheDirectory() as NSString)
                .expandingTildeInPath
            let ghcrDir = URL(fileURLWithPath: cacheDir).appendingPathComponent("ghcr")

            if FileManager.default.fileExists(atPath: ghcrDir.path) {
                try FileManager.default.removeItem(at: ghcrDir)
                try FileManager.default.createDirectory(
                    at: ghcrDir, withIntermediateDirectories: true)
                Logger.info("Successfully removed cached images")
            } else {
                Logger.info("No cached images found")
            }
        } catch {
            Logger.error("Failed to prune images", metadata: ["error": error.localizedDescription])
            throw error
        }
    }

    public struct ImageInfo: Codable {
        public let repository: String
        public let imageId: String  // This will be the shortened manifest ID
    }

    public struct ImageList: Codable {
        public let local: [ImageInfo]
        public let remote: [String]  // Keep this for future remote registry support
    }

    @MainActor
    public func getImages(organization: String = "trycua") async throws -> ImageList {
        Logger.info("Listing local images", metadata: ["organization": organization])

        let imageRegistry = try RegistryFactory.createRegistry(organization: organization)
        let cachedImages = try await imageRegistry.getImages()

        let imageInfos = cachedImages.map { image in
            ImageInfo(
                repository: image.repository,
                imageId: String(image.manifestId.prefix(12))
            )
        }

        ImagesPrinter.print(images: imageInfos.map { "\($0.repository):\($0.imageId)" })
        return ImageList(local: imageInfos, remote: [])
    }

    // MARK: - Settings Management

    public func getSettings() -> LumeSettings {
        return SettingsManager.shared.getSettings()
    }

    public func setHomeDirectory(_ path: String) throws {
        // Try to set the home directory in settings
        try SettingsManager.shared.setHomeDirectory(path: path)

        // Force recreate home instance to use the new path
        try home.validateHomeDirectory()

        Logger.info("Home directory updated", metadata: ["path": path])
    }

    // MARK: - VM Location Management

    public func addLocation(name: String, path: String) throws {
        Logger.info("Adding VM location", metadata: ["name": name, "path": path])

        try home.addLocation(name: name, path: path)

        Logger.info("VM location added successfully", metadata: ["name": name])
    }

    public func removeLocation(name: String) throws {
        Logger.info("Removing VM location", metadata: ["name": name])

        try home.removeLocation(name: name)

        Logger.info("VM location removed successfully", metadata: ["name": name])
    }

    public func setDefaultLocation(name: String) throws {
        Logger.info("Setting default VM location", metadata: ["name": name])

        try home.setDefaultLocation(name: name)

        Logger.info("Default VM location set successfully", metadata: ["name": name])
    }

    public func getLocations() -> [VMLocation] {
        return home.getLocations()
    }

    // MARK: - Cache Directory Management

    public func setCacheDirectory(path: String) throws {
        Logger.info("Setting cache directory", metadata: ["path": path])

        try SettingsManager.shared.setCacheDirectory(path: path)

        Logger.info("Cache directory updated", metadata: ["path": path])
    }

    public func getCacheDirectory() -> String {
        return SettingsManager.shared.getCacheDirectory()
    }

    public func isCachingEnabled() -> Bool {
        return SettingsManager.shared.isCachingEnabled()
    }

    public func setCachingEnabled(_ enabled: Bool) throws {
        Logger.info("Setting caching enabled", metadata: ["enabled": "\(enabled)"])

        try SettingsManager.shared.setCachingEnabled(enabled)

        Logger.info("Caching setting updated", metadata: ["enabled": "\(enabled)"])
    }

    // MARK: - Telemetry Management

    public func isTelemetryEnabled() -> Bool {
        return SettingsManager.shared.isTelemetryEnabled()
    }

    public func setTelemetryEnabled(_ enabled: Bool) throws {
        Logger.info("Setting telemetry enabled", metadata: ["enabled": "\(enabled)"])

        try SettingsManager.shared.setTelemetryEnabled(enabled)

        Logger.info("Telemetry setting updated", metadata: ["enabled": "\(enabled)"])
    }

    // MARK: - Private Helper Methods

    /// Normalizes a VM name by replacing colons with underscores
    private func normalizeVMName(name: String) -> String {
        let components = name.split(separator: ":")
        return components.count == 2 ? "\(components[0])_\(components[1])" : name
    }

    @MainActor
    private func createTempVMConfig(
        os: String,
        cpuCount: Int,
        memorySize: UInt64,
        diskSize: UInt64,
        display: String,
        networkMode: NetworkMode = .nat
    ) async throws -> VM {
        let config = try VMConfig(
            os: os,
            cpuCount: cpuCount,
            memorySize: memorySize,
            diskSize: diskSize,
            macAddress: VZMACAddress.randomLocallyAdministered().string,
            display: display,
            networkMode: networkMode
        )

        let vmDirContext = VMDirContext(
            dir: try home.createTempVMDirectory(),
            config: config,
            home: home,
            storage: nil
        )

        let imageLoader = os.lowercased() == "macos" ? imageLoaderFactory.createImageLoader() : nil
        return try vmFactory.createVM(vmDirContext: vmDirContext, imageLoader: imageLoader)
    }

    @MainActor
    private func loadVM(vmDir: VMDirectory, storage: String?) throws -> VM {
        // vmDir is now passed directly
        guard vmDir.initialized() else {
            throw VMError.notInitialized(vmDir.name) // Use name from vmDir
        }

        let config: VMConfig = try vmDir.loadConfig()
        // Pass the provided storage (which could be a path or named location)
        let vmDirContext = VMDirContext(
            dir: vmDir, config: config, home: home, storage: storage
        )

        let imageLoader =
            config.os.lowercased() == "macos" ? imageLoaderFactory.createImageLoader() : nil
        return try vmFactory.createVM(vmDirContext: vmDirContext, imageLoader: imageLoader)
    }

    // MARK: - Validation Methods

    private func validateCreateParameters(
        name: String, os: String, ipsw: String?, storage: String?
    ) throws {
        if os.lowercased() == "macos" {
            guard let ipsw = ipsw else {
                throw ValidationError("IPSW path required for macOS VM")
            }
            if ipsw != "latest" && !FileManager.default.fileExists(atPath: ipsw) {
                throw ValidationError("IPSW file not found")
            }
        } else if os.lowercased() == "linux" {
            if ipsw != nil {
                throw ValidationError("IPSW path not supported for Linux VM")
            }
        } else {
            throw ValidationError("Unsupported OS type: \(os)")
        }

        let vmDir: VMDirectory = try home.getVMDirectory(name, storage: storage)
        if vmDir.exists() {
            throw VMError.alreadyExists(name)
        }
    }

    private func validateSharedDirectories(_ directories: [SharedDirectory]) throws {
        for dir in directories {
            var isDirectory: ObjCBool = false
            guard FileManager.default.fileExists(atPath: dir.hostPath, isDirectory: &isDirectory),
                isDirectory.boolValue
            else {
                throw ValidationError(
                    "Host path does not exist or is not a directory: \(dir.hostPath)")
            }
        }
    }

    public func validateVMExists(_ name: String, storage: String? = nil) throws -> String? {
        // If location is specified, only check that location
        if let storage = storage {
            // Check if storage is a path by looking for directory separator
            if storage.contains("/") || storage.contains("\\") {
                // Treat as direct path
                let vmDir = try home.getVMDirectoryFromPath(name, storagePath: storage)
                guard vmDir.initialized() else {
                    throw VMError.notFound(name)
                }
                return storage  // Return the path as the location identifier
            } else {
                // Treat as named storage
                let vmDir = try home.getVMDirectory(name, storage: storage)
                guard vmDir.initialized() else {
                    throw VMError.notFound(name)
                }
                return storage
            }
        }

        // If no location specified, try to find the VM in any location
        let allVMs = try home.getAllVMDirectories()
        if let foundVM = allVMs.first(where: { $0.directory.name == name }) {
            // VM found, return its location
            return foundVM.locationName
        }

        // VM not found in any location
        throw VMError.notFound(name)
    }

    private func validateRunParameters(
        vmDir: VMDirectory, // Changed signature: accept VMDirectory
        sharedDirectories: [SharedDirectory]?,
        mount: Path?,
        usbMassStoragePaths: [Path]? = nil
    ) throws {
        // VM existence is confirmed by having vmDir, no need for validateVMExists
        if let dirs = sharedDirectories {
            try self.validateSharedDirectories(dirs)
        }

        // Validate USB mass storage paths
        if let usbPaths = usbMassStoragePaths {
            for path in usbPaths {
                if !FileManager.default.fileExists(atPath: path.path) {
                    throw ValidationError("USB mass storage image not found: \(path.path)")
                }
            }

            if #available(macOS 15.0, *) {
                // USB mass storage is supported
            } else {
                Logger.info(
                    "USB mass storage devices require macOS 15.0 or later. They will be ignored.")
            }
        }

        // Load config directly from vmDir
        let vmConfig = try vmDir.loadConfig()
        switch vmConfig.os.lowercased() {
        case "macos":
            if mount != nil {
                throw ValidationError(
                    "Mounting disk images is not supported for macOS VMs. If you are looking to mount a IPSW, please use the --ipsw option in the create command."
                )
            }
        case "linux":
            if let mount = mount, !FileManager.default.fileExists(atPath: mount.path) {
                throw ValidationError("Mount file not found: \(mount.path)")
            }
        default:
            break
        }
    }

    private func validatePullParameters(
        image: String,
        name: String,
        registry: String,
        organization: String,
        storage: String? = nil
    ) throws {
        guard !image.isEmpty else {
            throw ValidationError("Image name cannot be empty")
        }
        guard !name.isEmpty else {
            throw ValidationError("VM name cannot be empty")
        }
        guard !registry.isEmpty else {
            throw ValidationError("Registry cannot be empty")
        }
        guard !organization.isEmpty else {
            throw ValidationError("Organization cannot be empty")
        }

        // Determine if storage is a path or a named storage location
        let vmDir: VMDirectory
        if let storage = storage, storage.contains("/") || storage.contains("\\") {
            // Create the base directory if it doesn't exist
            if !FileManager.default.fileExists(atPath: storage) {
                Logger.info("Creating VM storage directory", metadata: ["path": storage])
                do {
                    try FileManager.default.createDirectory(
                        atPath: storage,
                        withIntermediateDirectories: true
                    )
                } catch {
                    throw HomeError.directoryCreationFailed(path: storage)
                }
            }
            
            // Use getVMDirectoryFromPath for direct paths
            vmDir = try home.getVMDirectoryFromPath(name, storagePath: storage)
        } else {
            // Use getVMDirectory for named storage locations
            vmDir = try home.getVMDirectory(name, storage: storage)
        }
        
        if vmDir.exists() {
            throw VMError.alreadyExists(name)
        }
    }

    private func validatePushParameters(
        name: String,
        imageName: String,
        tags: [String],
        registry: String,
        organization: String
    ) throws {
        guard !name.isEmpty else {
            throw ValidationError("VM name cannot be empty")
        }
        guard !imageName.isEmpty else {
            throw ValidationError("Image name cannot be empty")
        }
        guard !tags.isEmpty else {
            throw ValidationError("At least one tag must be provided.")
        }
        guard !registry.isEmpty else {
            throw ValidationError("Registry cannot be empty")
        }
        guard !organization.isEmpty else {
            throw ValidationError("Organization cannot be empty")
        }

        // Verify VM exists (this will throw if not found)
        _ = try self.validateVMExists(name)
    }

    // MARK: - Resize VM Disk

    @MainActor
    public func resize(
        name: String,
        compact: Bool = false,
        size: String? = nil,
        expandBy: String? = nil,
        storage: String? = nil,
        force: Bool = false
    ) async throws {
        let normalizedName = normalizeVMName(name: name)
        Logger.info(
            "Resizing VM disk",
            metadata: [
                "vm": normalizedName,
                "compact": "\\(compact)",
                "size": size ?? "nil",
                "expandBy": expandBy ?? "nil",
                "storage": storage ?? "default"
            ])

        do {
            // Validate VM exists
            let actualLocation = try self.validateVMExists(normalizedName, storage: storage)

            // Get the VM and check if it's running
            let vm = try get(name: normalizedName, storage: storage)
            if vm.details.status == "running" {
                Logger.error("Cannot resize a running VM", metadata: ["vm": normalizedName])
                throw VMError.alreadyRunning(normalizedName)
            }

            // Get current disk size for confirmation
            let currentDiskSize = try vm.getDiskSize()
            
            // Show confirmation unless force flag is set
            if !force {
                print("Current disk size: \\(formatBytes(currentDiskSize.total))")
                print("Used: \\(formatBytes(currentDiskSize.used))")
                
                if compact {
                    let potentialSavings = currentDiskSize.total - currentDiskSize.used
                    print("Potential space savings: ~\\(formatBytes(potentialSavings))")
                } else if let sizeStr = size {
                    print("New size: \\(sizeStr)")
                } else if let expandStr = expandBy {
                    print("Expanding by: \\(expandStr)")
                }
                
                print("\\nAre you sure you want to resize '\\(normalizedName)'? [y/N] ", terminator: "")
                guard let response = readLine()?.lowercased(),
                      response == "y" || response == "yes"
                else {
                    print("Resize cancelled")
                    return
                }
            }

            // Perform the resize operation
            if compact {
                Logger.info("Compacting disk", metadata: ["vm": normalizedName])
                try await vm.compactDisk()
                Logger.info("Disk compacted successfully", metadata: ["vm": normalizedName])
            } else if let sizeStr = size {
                let newSize = try parseSize(sizeStr)
                Logger.info("Resizing disk to absolute size", metadata: [
                    "vm": normalizedName,
                    "newSize": "\\(newSize)"
                ])
                try vm.resizeDisk(newSize)
                Logger.info("Disk resized successfully", metadata: ["vm": normalizedName])
            } else if let expandStr = expandBy {
                let expandAmount = try parseSize(expandStr)
                let newSize = currentDiskSize.total + expandAmount
                Logger.info("Expanding disk", metadata: [
                    "vm": normalizedName,
                    "current": "\\(currentDiskSize.total)",
                    "add": "\\(expandAmount)",
                    "new": "\\(newSize)"
                ])
                try vm.resizeDisk(newSize)
                Logger.info("Disk expanded successfully", metadata: ["vm": normalizedName])
            }

            // Show new size
            let newDiskSize = try vm.getDiskSize()
            print("\\nResize complete!")
            print("New disk size: \\(formatBytes(newDiskSize.total))")
            print("Used: \\(formatBytes(newDiskSize.used))")

        } catch {
            Logger.error("Failed to resize VM disk", metadata: ["error": error.localizedDescription])
            throw error
        }
    }

    // MARK: - Helper Functions

    /// Parse size string (e.g., "10GB", "512MB") to bytes
    private func parseSize(_ sizeStr: String) throws -> UInt64 {
        let trimmed = sizeStr.trimmingCharacters(in: .whitespaces).uppercased()
        
        // Extract number and unit
        var numberStr = ""
        var unit = ""
        
        for char in trimmed {
            if char.isNumber || char == "." {
                numberStr.append(char)
            } else {
                unit.append(char)
            }
        }
        
        guard let number = Double(numberStr) else {
            throw ValidationError("Invalid size format: \\(sizeStr)")
        }
        
        let multiplier: UInt64
        switch unit {
        case "B", "":
            multiplier = 1
        case "KB":
            multiplier = 1024
        case "MB":
            multiplier = 1024 * 1024
        case "GB", "G":
            multiplier = 1024 * 1024 * 1024
        case "TB":
            multiplier = 1024 * 1024 * 1024 * 1024
        default:
            throw ValidationError("Unknown size unit: \\(unit). Use B, KB, MB, GB, or TB")
        }
        
        return UInt64(number * Double(multiplier))
    }

    /// Format bytes to human-readable string
    private func formatBytes(_ bytes: UInt64) -> String {
        let formatter = ByteCountFormatter()
        formatter.allowedUnits = [.useGB, .useMB]
        formatter.countStyle = .file
        return formatter.string(fromByteCount: Int64(bytes))
    }
}

