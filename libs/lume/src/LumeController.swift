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
    @MainActor
    public func list() throws -> [VMDetails] {
        do {
            let vmLocations = try home.getAllVMDirectories()
            let statuses = try vmLocations.map { vmWithLoc in
                let vm = try self.get(
                    name: vmWithLoc.directory.name, locationName: vmWithLoc.locationName)
                return vm.details
            }
            return statuses
        } catch {
            Logger.error("Failed to list VMs", metadata: ["error": error.localizedDescription])
            throw error
        }
    }

    @MainActor
    public func clone(
        name: String, newName: String, sourceLocation: String? = nil, destLocation: String? = nil
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
            try self.validateVMExists(normalizedName, locationName: sourceLocation)

            // Copy the VM directory
            try home.copyVMDirectory(
                from: normalizedName,
                to: normalizedNewName,
                sourceLocation: sourceLocation,
                destLocation: destLocation
            )

            // Update MAC address in the cloned VM to ensure uniqueness
            let clonedVM = try get(name: normalizedNewName, locationName: destLocation)
            try clonedVM.setMacAddress(VZMACAddress.randomLocallyAdministered().string)

            Logger.info(
                "VM cloned successfully",
                metadata: ["source": normalizedName, "destination": normalizedNewName])
        } catch {
            Logger.error("Failed to clone VM", metadata: ["error": error.localizedDescription])
            throw error
        }
    }

    @MainActor
    public func get(name: String, locationName: String? = nil) throws -> VM {
        let normalizedName = normalizeVMName(name: name)
        do {
            try self.validateVMExists(normalizedName, locationName: locationName)

            let vm = try self.loadVM(name: normalizedName, locationName: locationName)
            return vm
        } catch {
            Logger.error("Failed to get VM", metadata: ["error": error.localizedDescription])
            throw error
        }
    }

    /// Factory for creating the appropriate VM type based on the OS
    @MainActor
    public func create(
        name: String,
        os: String,
        diskSize: UInt64,
        cpuCount: Int,
        memorySize: UInt64,
        display: String,
        ipsw: String?,
        locationName: String? = nil
    ) async throws {
        Logger.info(
            "Creating VM",
            metadata: [
                "name": name,
                "os": os,
                "location": locationName ?? "default",
                "disk_size": "\(diskSize / 1024 / 1024)MB",
                "cpu_count": "\(cpuCount)",
                "memory_size": "\(memorySize / 1024 / 1024)MB",
                "display": display,
                "ipsw": ipsw ?? "none",
            ])

        do {
            try validateCreateParameters(name: name, os: os, ipsw: ipsw, locationName: locationName)

            let vm = try await createTempVMConfig(
                os: os,
                cpuCount: cpuCount,
                memorySize: memorySize,
                diskSize: diskSize,
                display: display
            )

            try await vm.setup(
                ipswPath: ipsw ?? "none",
                cpuCount: cpuCount,
                memorySize: memorySize,
                diskSize: diskSize,
                display: display
            )

            try vm.finalize(to: name, home: home, locationName: locationName)

            Logger.info("VM created successfully", metadata: ["name": name])
        } catch {
            Logger.error("Failed to create VM", metadata: ["error": error.localizedDescription])
            throw error
        }
    }

    @MainActor
    public func delete(name: String, locationName: String? = nil) async throws {
        let normalizedName = normalizeVMName(name: name)
        Logger.info(
            "Deleting VM",
            metadata: [
                "name": normalizedName,
                "location": locationName ?? "default",
            ])

        do {
            try self.validateVMExists(normalizedName, locationName: locationName)

            // Stop VM if it's running
            if SharedVM.shared.getVM(name: normalizedName) != nil {
                try await stopVM(name: normalizedName)
            }

            let vmDir = try home.getVMDirectory(normalizedName, locationName: locationName)
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
        locationName: String? = nil
    ) throws {
        let normalizedName = normalizeVMName(name: name)
        Logger.info(
            "Updating VM settings",
            metadata: [
                "name": normalizedName,
                "location": locationName ?? "default",
                "cpu": cpu.map { "\($0)" } ?? "unchanged",
                "memory": memory.map { "\($0 / 1024 / 1024)MB" } ?? "unchanged",
                "disk_size": diskSize.map { "\($0 / 1024 / 1024)MB" } ?? "unchanged",
                "display": display ?? "unchanged",
            ])
        do {
            try self.validateVMExists(normalizedName, locationName: locationName)

            let vm = try get(name: normalizedName, locationName: locationName)

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
    public func stopVM(name: String) async throws {
        let normalizedName = normalizeVMName(name: name)
        Logger.info("Stopping VM", metadata: ["name": normalizedName])

        do {
            try self.validateVMExists(normalizedName)

            // Try to get VM from cache first
            let vm: VM
            if let cachedVM = SharedVM.shared.getVM(name: normalizedName) {
                vm = cachedVM
            } else {
                vm = try get(name: normalizedName)
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
        locationName: String? = nil
    ) async throws {
        let normalizedName = normalizeVMName(name: name)
        Logger.info(
            "Running VM",
            metadata: [
                "name": normalizedName,
                "location": locationName ?? "default",
                "no_display": "\(noDisplay)",
                "shared_directories":
                    "\(sharedDirectories.map( { $0.string } ).joined(separator: ", "))",
                "mount": mount?.path ?? "none",
                "vnc_port": "\(vncPort)",
                "recovery_mode": "\(recoveryMode)",
            ])

        do {
            // Check if this is an image reference (contains a tag)
            let components = name.split(separator: ":")
            if components.count == 2 {
                do {
                    try self.validateVMExists(normalizedName, locationName: locationName)
                } catch {
                    // If the VM doesn't exist, try to pull the image
                    try await pullImage(
                        image: name,
                        name: nil,
                        registry: registry,
                        organization: organization,
                        locationName: locationName
                    )
                }
            }

            try validateRunParameters(
                name: normalizedName,
                sharedDirectories: sharedDirectories,
                mount: mount,
                locationName: locationName
            )

            let vm = try get(name: normalizedName, locationName: locationName)
            SharedVM.shared.setVM(name: normalizedName, vm: vm)
            try await vm.run(
                noDisplay: noDisplay, sharedDirectories: sharedDirectories, mount: mount,
                vncPort: vncPort, recoveryMode: recoveryMode)
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
        locationName: String? = nil
    ) async throws {
        do {
            let vmName: String = name ?? normalizeVMName(name: image)

            Logger.info(
                "Pulling image",
                metadata: [
                    "image": image,
                    "name": name ?? "default",
                    "registry": registry,
                    "organization": organization,
                    "location": locationName ?? "default",
                ])

            try self.validatePullParameters(
                image: image,
                name: vmName,
                registry: registry,
                organization: organization,
                locationName: locationName
            )

            let imageContainerRegistry = ImageContainerRegistry(
                registry: registry, organization: organization)
            try await imageContainerRegistry.pull(
                image: image, name: vmName, locationName: locationName)

            Logger.info(
                "Setting new VM mac address",
                metadata: [
                    "vm_name": vmName,
                    "location": locationName ?? "default",
                ])

            // Update MAC address in the cloned VM to ensure uniqueness
            let vm = try get(name: vmName, locationName: locationName)
            try vm.setMacAddress(VZMACAddress.randomLocallyAdministered().string)

            Logger.info(
                "Image pulled successfully",
                metadata: [
                    "image": image,
                    "name": vmName,
                    "registry": registry,
                    "organization": organization,
                    "location": locationName ?? "default",
                ])
        } catch {
            Logger.error("Failed to pull image", metadata: ["error": error.localizedDescription])
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

        let imageContainerRegistry = ImageContainerRegistry(
            registry: "ghcr.io", organization: organization)
        let cachedImages = try await imageContainerRegistry.getImages()

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
        display: String
    ) async throws -> VM {
        let config = try VMConfig(
            os: os,
            cpuCount: cpuCount,
            memorySize: memorySize,
            diskSize: diskSize,
            macAddress: VZMACAddress.randomLocallyAdministered().string,
            display: display
        )

        let vmDirContext = VMDirContext(
            dir: try home.createTempVMDirectory(),
            config: config,
            home: home
        )

        let imageLoader = os.lowercased() == "macos" ? imageLoaderFactory.createImageLoader() : nil
        return try vmFactory.createVM(vmDirContext: vmDirContext, imageLoader: imageLoader)
    }

    @MainActor
    private func loadVM(name: String, locationName: String? = nil) throws -> VM {
        let vmDir = try home.getVMDirectory(name, locationName: locationName)
        guard vmDir.initialized() else {
            throw VMError.notInitialized(name)
        }

        let config: VMConfig = try vmDir.loadConfig()
        let vmDirContext = VMDirContext(dir: vmDir, config: config, home: home)

        let imageLoader =
            config.os.lowercased() == "macos" ? imageLoaderFactory.createImageLoader() : nil
        return try vmFactory.createVM(vmDirContext: vmDirContext, imageLoader: imageLoader)
    }

    // MARK: - Validation Methods

    private func validateCreateParameters(
        name: String, os: String, ipsw: String?, locationName: String?
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

        let vmDir = try home.getVMDirectory(name, locationName: locationName)
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

    public func validateVMExists(_ name: String, locationName: String? = nil) throws {
        let vmDir = try home.getVMDirectory(name, locationName: locationName)
        guard vmDir.initialized() else {
            throw VMError.notFound(name)
        }
    }

    private func validatePullParameters(
        image: String,
        name: String,
        registry: String,
        organization: String,
        locationName: String? = nil
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

        let vmDir = try home.getVMDirectory(name, locationName: locationName)
        if vmDir.exists() {
            throw VMError.alreadyExists(name)
        }
    }

    private func validateRunParameters(
        name: String, sharedDirectories: [SharedDirectory]?, mount: Path?,
        locationName: String? = nil
    ) throws {
        try self.validateVMExists(name, locationName: locationName)
        if let dirs = sharedDirectories {
            try self.validateSharedDirectories(dirs)
        }
        let vmConfig = try home.getVMDirectory(name, locationName: locationName).loadConfig()
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
}
