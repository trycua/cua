import Foundation
import Virtualization

/// Framework-agnostic VM configuration
struct VMVirtualizationServiceContext {
    let cpuCount: Int
    let memorySize: UInt64
    let display: String
    let sharedDirectories: [SharedDirectory]?
    let mount: Path?
    let hardwareModel: Data?
    let machineIdentifier: Data?
    let macAddress: String
    let diskPath: Path
    let nvramPath: Path
    let recoveryMode: Bool
    let usbMassStoragePaths: [Path]?
    let networkMode: NetworkMode
}

/// Protocol defining the interface for virtualization operations
@MainActor
protocol VMVirtualizationService {
    var state: VZVirtualMachine.State { get }
    /// The framework VM used by native display and VNC. Test services may return nil.
    var displayVirtualMachine: VZVirtualMachine? { get }
    func start() async throws
    func stop() async throws
    func pause() async throws
    func resume() async throws
    func updateSharedDirectories(_ sharedDirectories: [SharedDirectory]) async throws
    func waitForStop() async throws
}

/// Base implementation of VMVirtualizationService using VZVirtualMachine
@MainActor
class BaseVirtualizationService: VMVirtualizationService {
    final class VirtualMachineHandle: @unchecked Sendable {
        let machine: VZVirtualMachine
        let queue: DispatchQueue

        init(machine: VZVirtualMachine, queue: DispatchQueue) {
            self.machine = machine
            self.queue = queue
        }
    }

    let virtualMachineHandle: VirtualMachineHandle
    var virtualMachine: VZVirtualMachine { virtualMachineHandle.machine }
    let recoveryMode: Bool  // Store whether we should start in recovery mode
    private let lifecycleMonitor: VMVirtualMachineLifecycleMonitor

    var state: VZVirtualMachine.State {
        let handle = virtualMachineHandle
        return handle.queue.sync { handle.machine.state }
    }

    var displayVirtualMachine: VZVirtualMachine? {
        virtualMachine
    }

    init(
        virtualMachine: VZVirtualMachine,
        queue: DispatchQueue,
        recoveryMode: Bool = false
    ) {
        self.virtualMachineHandle = VirtualMachineHandle(machine: virtualMachine, queue: queue)
        self.recoveryMode = recoveryMode
        self.lifecycleMonitor = VMVirtualMachineLifecycleMonitor()
        let handle = virtualMachineHandle
        let monitor = lifecycleMonitor
        handle.queue.sync {
            handle.machine.delegate = monitor
        }
    }

    func start() async throws {
        let handle = virtualMachineHandle
        let recoveryMode = recoveryMode
        try await withCheckedThrowingContinuation {
            (continuation: CheckedContinuation<Void, Error>) in
            handle.queue.async {
                if #available(macOS 13, *) {
                    let startOptions = VZMacOSVirtualMachineStartOptions()
                    startOptions.startUpFromMacOSRecovery = recoveryMode
                    if recoveryMode {
                        Logger.info("Starting VM in recovery mode")
                    }
                    handle.machine.start(options: startOptions) { error in
                        if let error = error {
                            continuation.resume(throwing: error)
                        } else {
                            continuation.resume()
                        }
                    }
                } else {
                    Logger.info("Starting VM in normal mode")
                    handle.machine.start { result in
                        switch result {
                        case .success:
                            continuation.resume()
                        case .failure(let error):
                            continuation.resume(throwing: error)
                        }
                    }
                }
            }
        }
    }

    func stop() async throws {
        let handle = virtualMachineHandle
        let monitor = lifecycleMonitor
        try await withCheckedThrowingContinuation {
            (continuation: CheckedContinuation<Void, Error>) in
            handle.queue.async {
                handle.machine.stop { error in
                    if let error = error {
                        continuation.resume(throwing: error)
                    } else {
                        monitor.stoppedByHost()
                        continuation.resume()
                    }
                }
            }
        }
    }

    func pause() async throws {
        let handle = virtualMachineHandle
        try await withCheckedThrowingContinuation {
            (continuation: CheckedContinuation<Void, Error>) in
            handle.queue.async {
                handle.machine.pause { result in
                    switch result {
                    case .success:
                        continuation.resume()
                    case .failure(let error):
                        continuation.resume(throwing: error)
                    }
                }
            }
        }
    }

    func resume() async throws {
        let handle = virtualMachineHandle
        try await withCheckedThrowingContinuation {
            (continuation: CheckedContinuation<Void, Error>) in
            handle.queue.async {
                handle.machine.resume { result in
                    switch result {
                    case .success:
                        continuation.resume()
                    case .failure(let error):
                        continuation.resume(throwing: error)
                    }
                }
            }
        }
    }

    func updateSharedDirectories(_ sharedDirectories: [SharedDirectory]) async throws {
        let handle = virtualMachineHandle
        try await withCheckedThrowingContinuation {
            (continuation: CheckedContinuation<Void, Error>) in
            handle.queue.async {
                guard let device = handle.machine.directorySharingDevices.first(where: {
                    ($0 as? VZVirtioFileSystemDevice)?.tag
                        == VZVirtioFileSystemDeviceConfiguration.macOSGuestAutomountTag
                }) as? VZVirtioFileSystemDevice else {
                    continuation.resume(
                        throwing: VMError.internalError(
                            "The VM was not started with a live shared-folder device"))
                    return
                }
                device.share = Self.createDirectoryShare(sharedDirectories)
                continuation.resume()
            }
        }
    }

    func waitForStop() async throws {
        try await lifecycleMonitor.waitForStop()
    }

    // Helper methods for creating common configurations
    static func createStorageDeviceConfiguration(
        diskPath: Path,
        readOnly: Bool = false,
        cachingMode: VZDiskImageCachingMode = .automatic
    ) throws
        -> VZStorageDeviceConfiguration
    {
        return VZVirtioBlockDeviceConfiguration(
            attachment: try VZDiskImageStorageDeviceAttachment(
                url: diskPath.url,
                readOnly: readOnly,
                cachingMode: cachingMode,
                synchronizationMode: VZDiskImageSynchronizationMode.fsync
            )
        )
    }

    static func createUSBMassStorageDeviceConfiguration(
        diskPath: Path,
        readOnly: Bool = false,
        cachingMode: VZDiskImageCachingMode = .automatic
    )
        throws
        -> VZStorageDeviceConfiguration
    {
        if #available(macOS 15.0, *) {
            return VZUSBMassStorageDeviceConfiguration(
                attachment: try VZDiskImageStorageDeviceAttachment(
                    url: diskPath.url,
                    readOnly: readOnly,
                    cachingMode: cachingMode,
                    synchronizationMode: VZDiskImageSynchronizationMode.fsync
                )
            )
        } else {
            // Fallback to normal storage device if USB mass storage not available
            return try createStorageDeviceConfiguration(
                diskPath: diskPath, readOnly: readOnly, cachingMode: cachingMode)
        }
    }

    static func createNetworkDeviceConfiguration(
        macAddress: String,
        networkMode: NetworkMode = .nat
    ) throws -> VZNetworkDeviceConfiguration {
        let network = VZVirtioNetworkDeviceConfiguration()
        guard let vzMacAddress = VZMACAddress(string: macAddress) else {
            throw VMConfigError.invalidMachineIdentifier
        }

        switch networkMode {
        case .nat:
            network.attachment = VZNATNetworkDeviceAttachment()
        case .bridged(let interfaceName):
            let availableInterfaces = VZBridgedNetworkInterface.networkInterfaces
            guard let bridgeInterface = availableInterfaces.first(where: { iface in
                if let name = interfaceName {
                    return iface.identifier == name
                }
                // Auto-select: prefer the first active interface
                return true
            }) else {
                if let name = interfaceName {
                    let available = availableInterfaces.map { $0.identifier }.joined(separator: ", ")
                    throw VMConfigError.noBridgeInterfaceFound(
                        requested: name,
                        available: available.isEmpty ? "none" : available
                    )
                }
                throw VMConfigError.noBridgeInterfaceFound(
                    requested: nil,
                    available: "none"
                )
            }
            Logger.info(
                "Using bridged network interface",
                metadata: [
                    "interface": bridgeInterface.identifier,
                    "localizedName": bridgeInterface.localizedDisplayName ?? "unknown",
                ])
            network.attachment = VZBridgedNetworkDeviceAttachment(interface: bridgeInterface)
        }

        network.macAddress = vzMacAddress
        return network
    }

    static func createDirectorySharingDevices(sharedDirectories: [SharedDirectory]?)
        -> [VZDirectorySharingDeviceConfiguration]
    {
        let grouped = Dictionary(grouping: sharedDirectories ?? [], by: \.tag)
        return grouped.map { tag, directories in
            let device = VZVirtioFileSystemDeviceConfiguration(tag: tag)
            device.share = createDirectoryShare(directories)
            return device
        }
    }

    nonisolated static func createDirectoryShare(
        _ sharedDirectories: [SharedDirectory]
    ) -> VZDirectoryShare {
        var directories: [String: VZSharedDirectory] = [:]
        for sharedDirectory in sharedDirectories {
            let url = URL(fileURLWithPath: sharedDirectory.hostPath)
            let directory = VZSharedDirectory(url: url, readOnly: sharedDirectory.readOnly)
            let baseName = url.lastPathComponent.isEmpty ? "Shared Folder" : url.lastPathComponent
            var name = baseName
            var suffix = 2
            while directories[name] != nil {
                name = "\(baseName) (\(suffix))"
                suffix += 1
            }
            directories[name] = directory
        }
        return VZMultipleDirectoryShare(directories: directories)
    }
}

/// macOS-specific virtualization service
@MainActor
final class DarwinVirtualizationService: BaseVirtualizationService {
    static func createConfiguration(_ config: VMVirtualizationServiceContext) throws
        -> VZVirtualMachineConfiguration
    {
        let vzConfig = VZVirtualMachineConfiguration()
        vzConfig.cpuCount = config.cpuCount
        vzConfig.memorySize = config.memorySize

        // Platform configuration
        guard let machineIdentifier = config.machineIdentifier else {
            throw VMConfigError.emptyMachineIdentifier
        }

        guard let hardwareModel = config.hardwareModel else {
            throw VMConfigError.emptyHardwareModel
        }

        let platform = VZMacPlatformConfiguration()
        platform.auxiliaryStorage = VZMacAuxiliaryStorage(url: config.nvramPath.url)
        Logger.info("Pre-VZMacHardwareModel: hardwareModel=\(hardwareModel)")
        guard let vzHardwareModel = VZMacHardwareModel(dataRepresentation: hardwareModel) else {
            throw VMConfigError.invalidHardwareModel
        }
        platform.hardwareModel = vzHardwareModel
        guard
            let vzMachineIdentifier = VZMacMachineIdentifier(dataRepresentation: machineIdentifier)
        else {
            throw VMConfigError.invalidMachineIdentifier
        }
        platform.machineIdentifier = vzMachineIdentifier
        vzConfig.platform = platform
        vzConfig.bootLoader = VZMacOSBootLoader()

        // Use an explicit framebuffer configuration. Deriving the display from host
        // points and backing scale can leave VZVirtualMachineView with a black
        // framebuffer when native and VNC displays are attached concurrently.
        let display = VMDisplayResolution(string: config.display)!
        let graphics = VZMacGraphicsDeviceConfiguration()
        graphics.displays = [
            VZMacGraphicsDisplayConfiguration(
                widthInPixels: display.width,
                heightInPixels: display.height,
                pixelsPerInch: 220
            )
        ]
        vzConfig.graphicsDevices = [graphics]

        // Common configurations
        vzConfig.keyboards = [VZUSBKeyboardConfiguration()]
        vzConfig.pointingDevices = [VZUSBScreenCoordinatePointingDeviceConfiguration()]
        var storageDevices = [try createStorageDeviceConfiguration(diskPath: config.diskPath)]
        if let mount = config.mount {
            storageDevices.append(
                try createStorageDeviceConfiguration(diskPath: mount, readOnly: true))
        }
        // Add USB mass storage devices if specified
        if #available(macOS 15.0, *), let usbPaths = config.usbMassStoragePaths, !usbPaths.isEmpty {
            for usbPath in usbPaths {
                storageDevices.append(
                    try createUSBMassStorageDeviceConfiguration(diskPath: usbPath, readOnly: true))
            }
        }
        vzConfig.storageDevices = storageDevices
        vzConfig.networkDevices = [
            try createNetworkDeviceConfiguration(
                macAddress: config.macAddress,
                networkMode: config.networkMode
            )
        ]
        vzConfig.memoryBalloonDevices = [VZVirtioTraditionalMemoryBalloonDeviceConfiguration()]
        vzConfig.entropyDevices = [VZVirtioEntropyDeviceConfiguration()]
        
        // Audio configuration
        let soundDeviceConfiguration = VZVirtioSoundDeviceConfiguration()
        let inputAudioStreamConfiguration = VZVirtioSoundDeviceInputStreamConfiguration()
        let outputAudioStreamConfiguration = VZVirtioSoundDeviceOutputStreamConfiguration()
        
        inputAudioStreamConfiguration.source = VZHostAudioInputStreamSource()
        outputAudioStreamConfiguration.sink = VZHostAudioOutputStreamSink()
        
        soundDeviceConfiguration.streams = [inputAudioStreamConfiguration, outputAudioStreamConfiguration]
        vzConfig.audioDevices = [soundDeviceConfiguration]
        
        // Clipboard sharing via Spice agent
        let spiceAgentConsoleDevice = VZVirtioConsoleDeviceConfiguration()
        let spiceAgentPort = VZVirtioConsolePortConfiguration()
        spiceAgentPort.name = VZSpiceAgentPortAttachment.spiceAgentPortName
        let spiceAgentPortAttachment = VZSpiceAgentPortAttachment()
        spiceAgentPortAttachment.sharesClipboard = true
        spiceAgentPort.attachment = spiceAgentPortAttachment
        spiceAgentConsoleDevice.ports[0] = spiceAgentPort
        vzConfig.consoleDevices.append(spiceAgentConsoleDevice)

        // Optional serial console for debugging. When LUME_SERIAL_CONSOLE points
        // at a writable path, attach a virtio serial port whose output is written
        // there. Useful for observing early boot / recoveryOS behaviour that has
        // no other headless channel.
        if let serialLogPath = ProcessInfo.processInfo.environment["LUME_SERIAL_CONSOLE"],
            !serialLogPath.isEmpty
        {
            if !FileManager.default.fileExists(atPath: serialLogPath) {
                FileManager.default.createFile(atPath: serialLogPath, contents: nil)
            }
            if let writeHandle = FileHandle(forWritingAtPath: serialLogPath) {
                writeHandle.seekToEndOfFile()
                let serialAttachment = VZFileHandleSerialPortAttachment(
                    fileHandleForReading: nil,
                    fileHandleForWriting: writeHandle
                )
                // 1) Classic virtio serial port.
                let serialPort = VZVirtioConsoleDeviceSerialPortConfiguration()
                serialPort.attachment = serialAttachment
                vzConfig.serialPorts = [serialPort]
                // 2) A virtio console port marked isConsole, which designates it
                //    as the system console — some guests route console output here.
                let consoleDevice = VZVirtioConsoleDeviceConfiguration()
                let consolePort = VZVirtioConsolePortConfiguration()
                consolePort.isConsole = true
                consolePort.name = "lume.console"
                consolePort.attachment = serialAttachment
                consoleDevice.ports[0] = consolePort
                vzConfig.consoleDevices.append(consoleDevice)
                Logger.info("Attached serial console", metadata: ["path": serialLogPath])
            } else {
                Logger.error(
                    "Failed to open serial console log for writing",
                    metadata: ["path": serialLogPath])
            }
        }

        // Directory sharing. Keep an empty macOS automount device available so the
        // native toolbar can replace its share while the VM is running.
        var directorySharingDevices = createDirectorySharingDevices(
            sharedDirectories: config.sharedDirectories)
        let automountTag = VZVirtioFileSystemDeviceConfiguration.macOSGuestAutomountTag
        if !directorySharingDevices.contains(where: {
            ($0 as? VZVirtioFileSystemDeviceConfiguration)?.tag == automountTag
        }) {
            let device = VZVirtioFileSystemDeviceConfiguration(tag: automountTag)
            device.share = createDirectoryShare([])
            directorySharingDevices.append(device)
        }
        vzConfig.directorySharingDevices = directorySharingDevices

        // USB Controller configuration
        if #available(macOS 15.0, *) {
            let usbControllerConfiguration = VZXHCIControllerConfiguration()
            vzConfig.usbControllers = [usbControllerConfiguration]
        }

        try vzConfig.validate()
        return vzConfig
    }

    static func generateMacAddress() -> String {
        VZMACAddress.randomLocallyAdministered().string
    }

    static func generateMachineIdentifier() -> Data {
        VZMacMachineIdentifier().dataRepresentation
    }

    func createAuxiliaryStorage(at path: Path, hardwareModel: Data) throws {
        guard let vzHardwareModel = VZMacHardwareModel(dataRepresentation: hardwareModel) else {
            throw VMConfigError.invalidHardwareModel
        }
        _ = try VZMacAuxiliaryStorage(creatingStorageAt: path.url, hardwareModel: vzHardwareModel)
    }

    init(configuration: VMVirtualizationServiceContext) throws {
        let vzConfig = try Self.createConfiguration(configuration)
        let queue = DispatchQueue(
            label: "ai.cua.lume.virtual-machine.darwin",
            qos: .userInteractive
        )
        super.init(
            virtualMachine: VZVirtualMachine(configuration: vzConfig, queue: queue),
            queue: queue,
            recoveryMode: configuration.recoveryMode)
    }

    func installMacOS(imagePath: Path, progressHandler: (@Sendable (Double) -> Void)?) async throws
    {
        var observers: [NSKeyValueObservation] = []  // must hold observer references during installation to print process
        try await withCheckedThrowingContinuation {
            (continuation: CheckedContinuation<Void, Error>) in
            Task {
                let installer = VZMacOSInstaller(
                    virtualMachine: virtualMachine, restoringFromImageAt: imagePath.url)
                Logger.info("Starting macOS installation")

                if let progressHandler = progressHandler {
                    let observer = installer.progress.observe(
                        \.fractionCompleted, options: [.initial, .new]
                    ) { (progress, change) in
                        if let newValue = change.newValue {
                            progressHandler(newValue)
                        }
                    }
                    observers.append(observer)
                }

                installer.install { result in
                    switch result {
                    case .success:
                        continuation.resume()
                    case .failure(let error):
                        Logger.error("Failed to install, error=\(error))")
                        continuation.resume(throwing: error)
                    }
                }
            }
        }
        Logger.info("macOS installation finished")
    }
}

/// Linux-specific virtualization service
@MainActor
final class LinuxVirtualizationService: BaseVirtualizationService {
    static func createConfiguration(_ config: VMVirtualizationServiceContext) throws
        -> VZVirtualMachineConfiguration
    {
        let vzConfig = VZVirtualMachineConfiguration()
        vzConfig.cpuCount = config.cpuCount
        vzConfig.memorySize = config.memorySize

        // Platform configuration
        let platform = VZGenericPlatformConfiguration()
        if #available(macOS 15, *) {
            platform.isNestedVirtualizationEnabled =
                VZGenericPlatformConfiguration.isNestedVirtualizationSupported
        }
        vzConfig.platform = platform

        let bootLoader = VZEFIBootLoader()
        bootLoader.variableStore = VZEFIVariableStore(url: config.nvramPath.url)
        vzConfig.bootLoader = bootLoader

        // Graphics configuration
        let display = VMDisplayResolution(string: config.display)!
        let graphics = VZVirtioGraphicsDeviceConfiguration()
        graphics.scanouts = [
            VZVirtioGraphicsScanoutConfiguration(
                widthInPixels: display.width,
                heightInPixels: display.height
            )
        ]
        vzConfig.graphicsDevices = [graphics]

        // Common configurations
        vzConfig.keyboards = [VZUSBKeyboardConfiguration()]
        vzConfig.pointingDevices = [VZUSBScreenCoordinatePointingDeviceConfiguration()]
        // Use .cached caching mode for Linux VMs to prevent filesystem corruption.
        // The default .automatic mode causes EXT4/filesystem corruption in Linux guests
        // due to a known issue in Apple's Virtualization framework.
        // See: https://github.com/utmapp/UTM/pull/5919, https://github.com/lima-vm/lima/pull/2026
        let diskCachingMode = VZDiskImageCachingMode.cached
        var storageDevices = [try createStorageDeviceConfiguration(
            diskPath: config.diskPath, cachingMode: diskCachingMode)]
        if let mount = config.mount {
            storageDevices.append(
                try createStorageDeviceConfiguration(
                    diskPath: mount, readOnly: true, cachingMode: diskCachingMode))
        }
        // Add USB mass storage devices if specified
        if #available(macOS 15.0, *), let usbPaths = config.usbMassStoragePaths, !usbPaths.isEmpty {
            for usbPath in usbPaths {
                storageDevices.append(
                    try createUSBMassStorageDeviceConfiguration(
                        diskPath: usbPath, readOnly: true, cachingMode: diskCachingMode))
            }
        }
        vzConfig.storageDevices = storageDevices
        vzConfig.networkDevices = [
            try createNetworkDeviceConfiguration(
                macAddress: config.macAddress,
                networkMode: config.networkMode
            )
        ]
        vzConfig.memoryBalloonDevices = [VZVirtioTraditionalMemoryBalloonDeviceConfiguration()]
        vzConfig.entropyDevices = [VZVirtioEntropyDeviceConfiguration()]

        // Audio configuration
        let soundDeviceConfiguration = VZVirtioSoundDeviceConfiguration()
        let inputAudioStreamConfiguration = VZVirtioSoundDeviceInputStreamConfiguration()
        let outputAudioStreamConfiguration = VZVirtioSoundDeviceOutputStreamConfiguration()
        
        inputAudioStreamConfiguration.source = VZHostAudioInputStreamSource()
        outputAudioStreamConfiguration.sink = VZHostAudioOutputStreamSink()
        
        soundDeviceConfiguration.streams = [inputAudioStreamConfiguration, outputAudioStreamConfiguration]
        vzConfig.audioDevices = [soundDeviceConfiguration]

        // Clipboard sharing via Spice agent
        let spiceAgentConsoleDevice = VZVirtioConsoleDeviceConfiguration()
        let spiceAgentPort = VZVirtioConsolePortConfiguration()
        spiceAgentPort.name = VZSpiceAgentPortAttachment.spiceAgentPortName
        let spiceAgentPortAttachment = VZSpiceAgentPortAttachment()
        spiceAgentPortAttachment.sharesClipboard = true
        spiceAgentPort.attachment = spiceAgentPortAttachment
        spiceAgentConsoleDevice.ports[0] = spiceAgentPort
        vzConfig.consoleDevices.append(spiceAgentConsoleDevice)

        // Directory sharing
        var directorySharingDevices = createDirectorySharingDevices(
            sharedDirectories: config.sharedDirectories)

        // Add Rosetta support if available
        if #available(macOS 13.0, *) {
            if VZLinuxRosettaDirectoryShare.availability == .installed {
                do {
                    let rosettaShare = try VZLinuxRosettaDirectoryShare()
                    let rosettaDevice = VZVirtioFileSystemDeviceConfiguration(tag: "rosetta")
                    rosettaDevice.share = rosettaShare
                    directorySharingDevices.append(rosettaDevice)
                    Logger.info("Added Rosetta support to Linux VM")
                } catch {
                    Logger.info("Failed to add Rosetta support: \(error.localizedDescription)")
                }
            } else {
                Logger.info("Rosetta not installed, skipping Rosetta support")
            }
        }

        if !directorySharingDevices.isEmpty {
            vzConfig.directorySharingDevices = directorySharingDevices
        }

        // USB Controller configuration
        if #available(macOS 15.0, *) {
            let usbControllerConfiguration = VZXHCIControllerConfiguration()
            vzConfig.usbControllers = [usbControllerConfiguration]
        }

        try vzConfig.validate()
        return vzConfig
    }

    func generateMacAddress() -> String {
        VZMACAddress.randomLocallyAdministered().string
    }

    func createNVRAM(at path: Path) throws {
        _ = try VZEFIVariableStore(creatingVariableStoreAt: path.url)
    }

    init(configuration: VMVirtualizationServiceContext) throws {
        let vzConfig = try Self.createConfiguration(configuration)
        let queue = DispatchQueue(
            label: "ai.cua.lume.virtual-machine.linux",
            qos: .userInteractive
        )
        super.init(
            virtualMachine: VZVirtualMachine(configuration: vzConfig, queue: queue),
            queue: queue
        )
    }
}
