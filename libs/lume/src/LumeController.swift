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
                    // We need to check which subdirectories are valid VM dirs
                    let directoryURL = URL(fileURLWithPath: storage)
                    let contents = try FileManager.default.contentsOfDirectory(
                        at: directoryURL,
                        includingPropertiesForKeys: [.isDirectoryKey],
                        options: .skipsHiddenFiles
                    )
                    
                    let statuses = try contents.compactMap { subdir -> VMDetails? in
                        guard let isDirectory = try subdir.resourceValues(forKeys: [.isDirectoryKey]).isDirectory,
                              isDirectory else {
                            return nil
                        }
                        
                        let vmName = subdir.lastPathComponent
                        // Check if it's a valid VM directory
                        let vmDir = try home.getVMDirectoryFromPath(vmName, storagePath: storage)
                        if !vmDir.initialized() {
                            return nil
                        }
                        
                        do {
                            let vm = try self.get(name: vmName, storage: storage)
                            return vm.details
                        } catch {
                            // Skip invalid VM directories
                            return nil
                        }
                    }
                    return statuses
                } else {
                    // Named storage
                    let vmsWithLoc = try home.getAllVMDirectories()
                    let statuses = try vmsWithLoc.compactMap { vmWithLoc -> VMDetails? in
                        // Only include VMs from the specified location
                        if vmWithLoc.locationName != storage {
                            return nil
                        }
                        let vm = try self.get(
                            name: vmWithLoc.directory.name, storage: vmWithLoc.locationName)
                        return vm.details
                    }
                    return statuses
                }
            } else {
                // No storage filter - get all VMs
                let vmsWithLoc = try home.getAllVMDirectories()
                let statuses = try vmsWithLoc.compactMap { vmWithLoc -> VMDetails? in
                    let vm = try self.get(
                        name: vmWithLoc.directory.name, storage: vmWithLoc.locationName)
                    return vm.details
                }
                return statuses
            }
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
            // Validate source VM exists
            _ = try self.validateVMExists(normalizedName, storage: sourceLocation)

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
            try home.copyVMDirectory(
                from: normalizedName,
                to: normalizedNewName,
                sourceLocation: sourceLocation,
                destLocation: destLocation
            )

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
                    "vmName": normalizedName, "storage": storage ?? "default",
                    "error": error.localizedDescription,
                ])
            // Re-throw the original error to preserve its type
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
        storage: String? = nil
    ) async throws {
        Logger.info(
            "Creating VM",
            metadata: [
                "name": name,
                "os": os,
                "location": storage ?? "default",
                "disk_size": "\(diskSize / 1024 / 1024)MB",
                "cpu_count": "\(cpuCount)",
                "memory_size": "\(memorySize / 1024 / 1024)MB",
                "display": display,
                "ipsw": ipsw ?? "none",
            ])

        do {
            try validateCreateParameters(name: name, os: os, ipsw: ipsw, storage: storage)

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

            try vm.finalize(to: name, home: home, storage: storage)

            Logger.info("VM created successfully", metadata: ["name": name])
        } catch {
            Logger.error("Failed to create VM", metadata: ["error": error.localizedDescription])
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
                "location": storage ?? "default",
            ])

        do {
            // Find the actual location of the VM
            let actualLocation = try self.validateVMExists(
                normalizedName, storage: storage)

            // Stop VM if it's running
            if SharedVM.shared.getVM(name: normalizedName) != nil {
                try await stopVM(name: normalizedName)
            }

            let vmDir = try home.getVMDirectory(normalizedName, storage: actualLocation)
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
                "location": storage ?? "default",
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
        usbMassStoragePaths: [Path]? = nil
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
                "storage_param": storage ?? "default", // Log the original param
                "usb_storage_devices": "\(usbMassStoragePaths?.count ?? 0)",
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
                        "requested": storage ?? "default",
                        "actual": actualLocationName ?? "default",
                    ])
            }

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
                usbMassStoragePaths: usbMassStoragePaths)
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
            // Convert non-sparse image to sparse version if needed
            var actualImage = image
            var actualName = name

            // Split the image to get name and tag for both sparse and non-sparse cases
            let components = image.split(separator: ":")
            guard components.count == 2 else {
                throw ValidationError("Invalid image format. Expected format: name:tag")
            }

            let originalName = String(components[0])
            let tag = String(components[1])

            // For consistent VM naming, strip "-sparse" suffix if present when no name provided
            let normalizedBaseName: String
            if originalName.hasSuffix("-sparse") {
                normalizedBaseName = String(originalName.dropLast(7))  // drop "-sparse"
            } else {
                normalizedBaseName = originalName
            }

            // Set default VM name if not provided
            if actualName == nil {
                actualName = "\(normalizedBaseName)_\(tag)"
            }

            // Convert non-sparse image to sparse version if needed
            if !image.contains("-sparse") {
                // Create sparse version of the image name
                actualImage = "\(originalName)-sparse:\(tag)"

                Logger.info(
                    "Converting to sparse image",
                    metadata: [
                        "original": image,
                        "sparse": actualImage,
                        "vm_name": actualName ?? "default",
                    ]
                )
            }

            let vmName = actualName ?? "default"  // Just use actualName as it's already normalized

            Logger.info(
                "Pulling image",
                metadata: [
                    "image": actualImage,
                    "name": vmName,
                    "registry": registry,
                    "organization": organization,
                    "location": storage ?? "default",
                ])

            try self.validatePullParameters(
                image: actualImage,
                name: vmName,
                registry: registry,
                organization: organization,
                storage: storage
            )

            let imageContainerRegistry = ImageContainerRegistry(
                registry: registry, organization: organization)
            let _ = try await imageContainerRegistry.pull(
                image: actualImage,
                name: vmName,
                locationName: storage)

            Logger.info(
                "Setting new VM mac address",
                metadata: [
                    "vm_name": vmName,
                    "location": storage ?? "default",
                ])

            // Update MAC address in the cloned VM to ensure uniqueness
            let vm = try get(name: vmName, storage: storage)
            try vm.setMacAddress(VZMACAddress.randomLocallyAdministered().string)

            Logger.info(
                "Image pulled successfully",
                metadata: [
                    "image": actualImage,
                    "name": vmName,
                    "registry": registry,
                    "organization": organization,
                    "location": storage ?? "default",
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
                    "location": storage ?? "default",
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

            // Use ImageContainerRegistry to push the VM
            let imageContainerRegistry = ImageContainerRegistry(
                registry: registry, organization: organization)

            try await imageContainerRegistry.push(
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

    public func isCachingEnabled() -> Bool {
        return SettingsManager.shared.isCachingEnabled()
    }

    public func setCachingEnabled(_ enabled: Bool) throws {
        Logger.info("Setting caching enabled", metadata: ["enabled": "\(enabled)"])

        try SettingsManager.shared.setCachingEnabled(enabled)

        Logger.info("Caching setting updated", metadata: ["enabled": "\(enabled)"])
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
}
