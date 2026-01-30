import Foundation

/// Manages the application's home directory and virtual machine directories.
/// Responsible for creating, accessing, and validating the application's directory structure.
final class Home {
    // MARK: - Constants

    private enum Constants {
        static let defaultDirectoryName = ".lume"
        static let homeDirPath = "~/\(defaultDirectoryName)"
    }

    // MARK: - Properties

    private var _homeDir: Path
    private let settingsManager: SettingsManager
    private let fileManager: FileManager
    private var locations: [String: VMLocation] = [:]

    // Current home directory based on default location
    var homeDir: Path {
        return _homeDir
    }

    // MARK: - Initialization

    init(
        settingsManager: SettingsManager = SettingsManager.shared,
        fileManager: FileManager = .default
    ) {
        self.settingsManager = settingsManager
        self.fileManager = fileManager

        // Get home directory path from settings or use default
        let settings = settingsManager.getSettings()
        guard let defaultLocation = settings.defaultLocation else {
            fatalError("No default VM location found")
        }

        self._homeDir = Path(defaultLocation.path)

        // Cache all locations
        for location in settings.vmLocations {
            locations[location.name] = location
        }
    }

    // MARK: - VM Directory Management

    /// Creates a temporary VM directory with a unique identifier
    /// - Returns: A VMDirectory instance representing the created directory
    /// - Throws: HomeError if directory creation fails
    func createTempVMDirectory() throws -> VMDirectory {
        let uuid = UUID().uuidString
        let tempDir = homeDir.directory(uuid)

        Logger.info("Creating temporary directory", metadata: ["path": tempDir.path])

        do {
            try createDirectory(at: tempDir.url)
            return VMDirectory(tempDir)
        } catch {
            throw HomeError.directoryCreationFailed(path: tempDir.path)
        }
    }

    /// Gets a VM directory for a specific VM name and optional location
    ///
    /// - Parameters:
    ///   - name: Name of the VM directory
    ///   - storage: Optional name of the VM location (default: default location)
    /// - Returns: A VMDirectory instance
    /// - Throws: HomeError if location not found
    func getVMDirectory(_ name: String, storage: String? = nil) throws -> VMDirectory {
        // Special case for ephemeral storage using macOS temporary directory
        if let storage = storage, storage == "ephemeral" {
            // Get the current temporary directory
            let tmpDir = ProcessInfo.processInfo.environment["TMPDIR"] ?? "/tmp"
            // Remove trailing slash if present
            let cleanPath = tmpDir.hasSuffix("/") ? String(tmpDir.dropLast()) : tmpDir
            
            // Create the directory if it doesn't exist
            if !fileExists(at: cleanPath) {
                try createVMLocation(at: cleanPath)
            }
            
            let baseDir = Path(cleanPath)
            return VMDirectory(baseDir.directory(name))
        }

        // Check if storage is a direct path
        if let storage = storage, (storage.contains("/") || storage.contains("\\")) {
            let cleanPath = storage.hasSuffix("/") ? String(storage.dropLast()) : storage
            let baseDir = Path(cleanPath)
            return VMDirectory(baseDir.directory(name))
        }

        let location: VMLocation

        if let storage = storage {
            // Get a specific location
            guard let loc = locations[storage] else {
                throw VMLocationError.locationNotFound(name: storage)
            }
            location = loc
        } else {
            // Use default location
            let settings = settingsManager.getSettings()
            guard let defaultLocation = settings.defaultLocation else {
                throw HomeError.invalidHomeDirectory
            }
            location = defaultLocation
        }

        let baseDir = Path(location.expandedPath)
        return VMDirectory(baseDir.directory(name))
    }
    
    /// Gets a VM directory from a direct file path
    ///
    /// - Parameters:
    ///   - name: Name of the VM directory
    ///   - storagePath: Direct file system path where the VM is located
    /// - Returns: A VMDirectory instance
    /// - Throws: HomeError if path is invalid
    func getVMDirectoryFromPath(_ name: String, storagePath: String) throws -> VMDirectory {
        let baseDir = Path(storagePath)
        
        // Create the directory if it doesn't exist
        if !fileExists(at: storagePath) {
            Logger.info("Creating storage directory", metadata: ["path": storagePath])
            try createVMLocation(at: storagePath)
        } else if !isValidDirectory(at: storagePath) {
            // Path exists but isn't a valid directory
            throw HomeError.invalidHomeDirectory
        }
        
        return VMDirectory(baseDir.directory(name))
    }

    /// Returns all initialized VM directories across all locations
    /// - Returns: An array of VMDirectory instances with location info
    /// - Throws: HomeError if directory access is denied
    func getAllVMDirectories() throws -> [VMDirectoryWithLocation] {
        var results: [VMDirectoryWithLocation] = []

        // Loop through all locations
        let settings = settingsManager.getSettings()

        // Note: We intentionally do NOT scan TMPDIR for ephemeral VMs.
        // Ephemeral VMs are short-lived and should be explicitly managed by the code
        // that creates them, not automatically discovered. Scanning TMPDIR would pick up
        // test leftovers and other temporary directories that happen to look like VMs.

        for location in settings.vmLocations {
            let locationPath = Path(location.expandedPath)

            // Skip non-existent locations
            if !locationPath.exists() {
                continue
            }

            do {
                let allFolders = try fileManager.contentsOfDirectory(
                    at: locationPath.url,
                    includingPropertiesForKeys: nil
                )

                let folders =
                    allFolders
                    .compactMap { url in
                        let sanitizedName = sanitizeFileName(url.lastPathComponent)
                        let dir = VMDirectory(locationPath.directory(sanitizedName))
                        let dirWithLoc =
                            dir.initialized()
                            ? VMDirectoryWithLocation(directory: dir, locationName: location.name)
                            : nil
                        return dirWithLoc
                    }

                results.append(contentsOf: folders)
            } catch {
                Logger.error(
                    "Failed to access VM location",
                    metadata: [
                        "location": location.name,
                        "error": error.localizedDescription,
                    ])
                // Continue to next location rather than failing completely
            }
        }

        return results
    }

    /// Copies a VM directory to a new location with a new name
    /// - Parameters:
    ///   - sourceName: Name of the source VM
    ///   - destName: Name for the destination VM
    ///   - sourceLocation: Optional name of the source location
    ///   - destLocation: Optional name of the destination location
    /// - Throws: HomeError if the copy operation fails
    func copyVMDirectory(
        from sourceName: String,
        to destName: String,
        sourceLocation: String? = nil,
        destLocation: String? = nil
    ) throws {
        let sourceDir = try getVMDirectory(sourceName, storage: sourceLocation)
        let destDir = try getVMDirectory(destName, storage: destLocation)

        // Check if destination directory exists at all
        if destDir.exists() {
            throw HomeError.directoryAlreadyExists(path: destDir.dir.path)
        }

        do {
            try fileManager.copyItem(atPath: sourceDir.dir.path, toPath: destDir.dir.path)
        } catch {
            throw HomeError.directoryCreationFailed(path: destDir.dir.path)
        }
    }

    /// Copies a VM directory manually to allow for disk modifications
    /// - Parameters:
    ///   - sourceName: Name of the source VM
    ///   - destName: Name for the destination VM
    ///   - sourceLocation: Optional name of the source location
    ///   - destLocation: Optional name of the destination location
    ///   - compact: Whether to simple-compact the disk (skip zeros)
    /// - Throws: HomeError or VMDirectoryError
    func copyVMDirectoryManual(
        from sourceName: String,
        to destName: String,
        sourceLocation: String? = nil,
        destLocation: String? = nil,
        compact: Bool = false
    ) throws {
        let sourceDir = try getVMDirectory(sourceName, storage: sourceLocation)
        let destDir = try getVMDirectory(destName, storage: destLocation)

        // Check if destination directory exists at all
        if destDir.exists() {
            throw HomeError.directoryAlreadyExists(path: destDir.dir.path)
        }

        // Create destination directory
        try createDirectory(at: destDir.dir.url)

        // Copy auxiliary files (config, nvram, provisioning marker if exists)
        // We skip 'sessions.json' as cloning shouldn't copy running session info
        let filesToCopy = [
            sourceDir.configPath,
            sourceDir.nvramPath,
            sourceDir.provisioningPath
        ]

        for srcPath in filesToCopy {
            if srcPath.exists() {
                let dstPath = destDir.dir.file(srcPath.url.lastPathComponent)
                try fileManager.copyItem(at: srcPath.url, to: dstPath.url)
            }
        }

        // Handle Disk Copy
        if compact {
            Logger.info("Compacting disk image during clone...", metadata: ["source": sourceName])
            try sourceDir.compactCopyDisk(to: destDir.diskPath)
        } else {
            // Standard copy if not compacting (but manual flow)
            if sourceDir.diskPath.exists() {
                try fileManager.copyItem(at: sourceDir.diskPath.url, to: destDir.diskPath.url)
            }
        }
    }

    // MARK: - Location Management

    /// Adds a new VM location
    /// - Parameters:
    ///   - name: Location name
    ///   - path: Location path
    /// - Throws: Error if location cannot be added
    func addLocation(name: String, path: String) throws {
        let location = VMLocation(name: name, path: path)
        try settingsManager.addLocation(location)

        // Update cache
        locations[name] = location
    }

    /// Removes a VM location
    /// - Parameter name: Location name
    /// - Throws: Error if location cannot be removed
    func removeLocation(name: String) throws {
        try settingsManager.removeLocation(name: name)

        // Update cache
        locations.removeValue(forKey: name)
    }

    /// Sets the default VM location
    /// - Parameter name: Location name
    /// - Throws: Error if location cannot be set as default
    func setDefaultLocation(name: String) throws {
        try settingsManager.setDefaultLocation(name: name)

        // Update home directory
        guard let location = locations[name] else {
            throw VMLocationError.locationNotFound(name: name)
        }

        // Update homeDir to reflect the new default
        self._homeDir = Path(location.path)
    }

    /// Gets all available VM locations
    /// - Returns: Array of VM locations
    func getLocations() -> [VMLocation] {
        return settingsManager.getSettings().sortedLocations
    }

    /// Gets the default VM location
    /// - Returns: Default VM location
    /// - Throws: HomeError if no default location
    func getDefaultLocation() throws -> VMLocation {
        guard let location = settingsManager.getSettings().defaultLocation else {
            throw HomeError.invalidHomeDirectory
        }
        return location
    }

    // MARK: - Directory Validation

    /// Validates and ensures the existence of all VM locations
    /// - Throws: HomeError if validation fails or directory creation fails
    func validateHomeDirectory() throws {
        let settings = settingsManager.getSettings()

        for location in settings.vmLocations {
            let path = location.expandedPath
            if !fileExists(at: path) {
                try createVMLocation(at: path)
            } else if !isValidDirectory(at: path) {
                throw HomeError.invalidHomeDirectory
            }
        }
    }

    // MARK: - Private Helpers

    private func createVMLocation(at path: String) throws {
        do {
            try fileManager.createDirectory(
                atPath: path,
                withIntermediateDirectories: true
            )
        } catch {
            throw HomeError.directoryCreationFailed(path: path)
        }
    }

    private func createDirectory(at url: URL) throws {
        try fileManager.createDirectory(
            at: url,
            withIntermediateDirectories: true
        )
    }

    private func isValidDirectory(at path: String) -> Bool {
        var isDirectory: ObjCBool = false
        return fileManager.fileExists(atPath: path, isDirectory: &isDirectory)
            && isDirectory.boolValue
            && Path(path).writable()
    }

    private func fileExists(at path: String) -> Bool {
        return fileManager.fileExists(atPath: path)
    }

    private func sanitizeFileName(_ name: String) -> String {
        // Only decode percent encoding (e.g., %20 for spaces)
        return name.removingPercentEncoding ?? name
    }
}

// MARK: - VM Directory with Location

/// Represents a VM directory with its location information
struct VMDirectoryWithLocation {
    let directory: VMDirectory
    let locationName: String
}

// MARK: - Home + CustomStringConvertible

extension Home: CustomStringConvertible {
    var description: String {
        "Home(path: \(homeDir.path))"
    }
}
