import Foundation

// MARK: - VMDirectory

/// Manages a virtual machine's directory structure and files
/// Responsible for:
/// - Managing VM configuration files
/// - Handling disk operations
/// - Managing VM state and locking
/// - Providing access to VM-related paths
struct VMDirectory: Sendable {
    // MARK: - Constants
    
    private enum FileNames {
        static let nvram = "nvram.bin"
        static let disk = "disk.img"
        static let config = "config.json"
        static let sessions = "sessions.json"
        static let provisioning = ".provisioning"
    }
    
    // MARK: - Properties
    
    let dir: Path
    let nvramPath: Path
    let diskPath: Path
    let configPath: Path
    let sessionsPath: Path
    let provisioningPath: Path
    
    /// The name of the VM directory
    var name: String { dir.name }
    
    // MARK: - Initialization
    
    /// Creates a new VMDirectory instance
    /// - Parameters:
    ///   - dir: The base directory path for the VM
    init(_ dir: Path) {
        self.dir = dir
        self.nvramPath = dir.file(FileNames.nvram)
        self.diskPath = dir.file(FileNames.disk)
        self.configPath = dir.file(FileNames.config)
        self.sessionsPath = dir.file(FileNames.sessions)
        self.provisioningPath = dir.file(FileNames.provisioning)
    }
}

// MARK: - VM State Management

extension VMDirectory {
    /// Checks if the VM directory is initialized (either fully ready or provisioning)
    /// A VM is considered initialized if it has:
    /// - All required files (config + disk + nvram), OR
    /// - Config + provisioning marker (VM is being created)
    func initialized() -> Bool {
        let configExists = configPath.exists()
        let diskExists = diskPath.exists()
        let nvramExists = nvramPath.exists()
        let provisioningExists = provisioningPath.exists()

        // Fully initialized VM
        if configExists && diskExists && nvramExists {
            return true
        }

        // VM being provisioned (has config and provisioning marker)
        if configExists && provisioningExists {
            return true
        }

        return false
    }

    /// Checks if the VM directory exists
    func exists() -> Bool {
        dir.exists()
    }
}

// MARK: - Disk Management

extension VMDirectory {
    /// Resizes the VM's disk to the specified size
    /// - Parameter size: The new size in bytes
    /// - Throws: VMDirectoryError if the disk operation fails
    func setDisk(_ size: UInt64) throws {
        do {
            if !diskPath.exists() {
                guard FileManager.default.createFile(atPath: diskPath.path, contents: nil) else {
                    throw VMDirectoryError.fileCreationFailed(diskPath.path)
                }
            }
            
            let handle = try FileHandle(forWritingTo: diskPath.url)
            defer { try? handle.close() }
            
            try handle.truncate(atOffset: size)
        } catch {
        }
    }
}

// MARK: - Configuration Management

extension VMDirectory {
    /// Saves the VM configuration to disk
    /// - Parameter config: The configuration to save
    /// - Throws: VMDirectoryError if the save operation fails
    func saveConfig(_ config: VMConfig) throws {
        let encoder = JSONEncoder()
        encoder.outputFormatting = .prettyPrinted
        
        do {
            let data = try encoder.encode(config)
            guard FileManager.default.createFile(atPath: configPath.path, contents: data) else {
                throw VMDirectoryError.fileCreationFailed(configPath.path)
            }
        } catch {
            throw VMDirectoryError.invalidConfigData
        }
    }

    /// Loads the VM configuration from disk
    /// - Returns: The loaded configuration
    /// - Throws: VMDirectoryError if the load operation fails
    func loadConfig() throws -> VMConfig {
        guard let data = FileManager.default.contents(atPath: configPath.path) else {
            throw VMDirectoryError.configNotFound
        }
        
        do {
            let decoder = JSONDecoder()
            return try decoder.decode(VMConfig.self, from: data)
        } catch {
            throw VMDirectoryError.invalidConfigData
        }
    }
}

// MARK: - VNC Session Management

struct VNCSession: Codable {
    let url: String
    let sharedDirectories: [SharedDirectory]?
    
    init(url: String, sharedDirectories: [SharedDirectory]? = nil) {
        self.url = url
        self.sharedDirectories = sharedDirectories
    }
}

extension VMDirectory {
    /// Saves VNC session information to disk
    /// - Parameters:
    ///   - session: The VNC session to save
    ///   - sharedDirectories: Optional array of shared directories to save with the session
    /// - Throws: VMDirectoryError if the save operation fails
    func saveSession(_ session: VNCSession) throws {
        let encoder = JSONEncoder()
        encoder.outputFormatting = .prettyPrinted
        
        do {
            let data = try encoder.encode(session)
            guard FileManager.default.createFile(atPath: sessionsPath.path, contents: data) else {
                throw VMDirectoryError.fileCreationFailed(sessionsPath.path)
            }
        } catch {
            throw VMDirectoryError.invalidSessionData
        }
    }
    
    /// Loads the VNC session information from disk
    /// - Returns: The loaded VNC session
    /// - Throws: VMDirectoryError if the load operation fails
    func loadSession() throws -> VNCSession {
        guard let data = FileManager.default.contents(atPath: sessionsPath.path) else {
            throw VMDirectoryError.sessionNotFound
        }
        
        do {
            let decoder = JSONDecoder()
            return try decoder.decode(VNCSession.self, from: data)
        } catch {
            throw VMDirectoryError.invalidSessionData
        }
    }
    
    /// Removes the VNC session information from disk
    func clearSession() {
        try? FileManager.default.removeItem(atPath: sessionsPath.path)
    }
}

// MARK: - Provisioning Marker Management

/// Represents the provisioning state of a VM during long-running operations
struct ProvisioningMarker: Codable {
    /// The type of operation being performed (e.g., "ipsw_install", "unattended_setup")
    let operation: String
    /// When the provisioning started (Unix timestamp)
    let startedAt: Double
    
    init(operation: String) {
        self.operation = operation
        self.startedAt = Date().timeIntervalSince1970
    }
    
    /// Returns true if provisioning started more than the specified hours ago
    func isStale(hours: Double = 8.0) -> Bool {
        let elapsed = Date().timeIntervalSince1970 - startedAt
        return elapsed > (hours * 3600)
    }
}

extension VMDirectory {
    /// Saves a provisioning marker to indicate the VM is being created/configured
    /// - Parameter marker: The provisioning marker containing operation type
    /// - Throws: VMDirectoryError if the save operation fails
    func saveProvisioningMarker(_ marker: ProvisioningMarker) throws {
        let encoder = JSONEncoder()
        encoder.outputFormatting = .prettyPrinted

        do {
            let data = try encoder.encode(marker)
            guard FileManager.default.createFile(atPath: provisioningPath.path, contents: data) else {
                throw VMDirectoryError.fileCreationFailed(provisioningPath.path)
            }
        } catch {
            throw VMDirectoryError.fileCreationFailed(provisioningPath.path)
        }
    }

    /// Loads the provisioning marker if it exists
    /// - Returns: The provisioning marker, or nil if not in provisioning state
    func loadProvisioningMarker() -> ProvisioningMarker? {
        guard let data = FileManager.default.contents(atPath: provisioningPath.path) else {
            return nil
        }
        return try? JSONDecoder().decode(ProvisioningMarker.self, from: data)
    }

    /// Removes the provisioning marker after creation completes
    func clearProvisioningMarker() {
        try? FileManager.default.removeItem(atPath: provisioningPath.path)
    }

    /// Checks if the VM is currently being provisioned
    func isProvisioning() -> Bool {
        provisioningPath.exists()
    }
}

// MARK: - CustomStringConvertible
extension VMDirectory: CustomStringConvertible {
    var description: String {
        "VMDirectory(path: \(dir.path))"
    }
}

extension VMDirectory {
    func delete() throws {
        try FileManager.default.removeItem(atPath: dir.path)
    }
}

// MARK: - Lightweight VM Details

extension VMDirectory {
    /// Get disk size information without loading the full VM
    func getDiskSize() -> DiskSize {
        do {
            let resourceValues = try diskPath.url.resourceValues(forKeys: [
                .totalFileAllocatedSizeKey,
                .totalFileSizeKey,
            ])

            if let allocated = resourceValues.totalFileAllocatedSize,
               let total = resourceValues.totalFileSize {
                return DiskSize(allocated: UInt64(allocated), total: UInt64(total))
            }
        } catch {
            // Fallback to config value
        }

        // Try to get from config as fallback
        if let config = try? loadConfig() {
            return DiskSize(allocated: 0, total: config.diskSize ?? 0)
        }

        return DiskSize(allocated: 0, total: 0)
    }

    /// Build VMDetails directly without instantiating a full VM object
    /// This is much faster for listing VMs since it avoids:
    /// - Creating ImageLoader instances
    /// - Initializing virtualization services
    /// - Building the full VM object graph
    ///
    /// - Parameters:
    ///   - locationName: The storage location name for this VM
    ///   - status: The VM status ("running", "stopped", or "provisioning")
    ///   - provisioningOperation: Optional operation type if status is "provisioning"
    ///   - vncUrl: Optional VNC URL if running
    ///   - ipAddress: Optional IP address if running
    ///   - sshAvailable: Optional SSH availability status
    /// - Returns: VMDetails or nil if config cannot be loaded
    func getDetails(
        locationName: String,
        status: String,
        provisioningOperation: String? = nil,
        vncUrl: String?,
        ipAddress: String?,
        sshAvailable: Bool? = nil
    ) -> VMDetails? {
        guard let config = try? loadConfig() else {
            return nil
        }

        return VMDetails(
            name: name,
            os: config.os,
            cpuCount: config.cpuCount ?? 0,
            memorySize: config.memorySize ?? 0,
            diskSize: getDiskSize(),
            display: config.display.string,
            status: status,
            provisioningOperation: provisioningOperation,
            vncUrl: vncUrl,
            ipAddress: ipAddress,
            sshAvailable: sshAvailable,
            locationName: locationName,
            sharedDirectories: nil,
            networkMode: config.networkMode.description
        )
    }
}
