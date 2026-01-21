import AppKit
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
}

/// Protocol defining the interface for virtualization operations
@MainActor
protocol VMVirtualizationService {
    var state: VZVirtualMachine.State { get }
    func start() async throws
    func stop() async throws
    func pause() async throws
    func resume() async throws
    func getVirtualMachine() -> Any
}

/// Base implementation of VMVirtualizationService using VZVirtualMachine
@MainActor
class BaseVirtualizationService: VMVirtualizationService {
    let virtualMachine: VZVirtualMachine
    let recoveryMode: Bool  // Store whether we should start in recovery mode

    var state: VZVirtualMachine.State {
        virtualMachine.state
    }

    init(virtualMachine: VZVirtualMachine, recoveryMode: Bool = false) {
        self.virtualMachine = virtualMachine
        self.recoveryMode = recoveryMode
    }

    func start() async throws {
        try await withCheckedThrowingContinuation {
            (continuation: CheckedContinuation<Void, Error>) in
            Task { @MainActor in
                if #available(macOS 13, *) {
                    let startOptions = VZMacOSVirtualMachineStartOptions()
                    startOptions.startUpFromMacOSRecovery = recoveryMode
                    if recoveryMode {
                        Logger.info("Starting VM in recovery mode")
                    }
                    virtualMachine.start(options: startOptions) { error in
                        if let error = error {
                            continuation.resume(throwing: error)
                        } else {
                            continuation.resume()
                        }
                    }
                } else {
                    Logger.info("Starting VM in normal mode")
                    virtualMachine.start { result in
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
        try await withCheckedThrowingContinuation {
            (continuation: CheckedContinuation<Void, Error>) in
            virtualMachine.stop { error in
                if let error = error {
                    continuation.resume(throwing: error)
                } else {
                    continuation.resume()
                }
            }
        }
    }

    func pause() async throws {
        try await withCheckedThrowingContinuation {
            (continuation: CheckedContinuation<Void, Error>) in
            virtualMachine.start { result in
                switch result {
                case .success:
                    continuation.resume()
                case .failure(let error):
                    continuation.resume(throwing: error)
                }
            }
        }
    }

    func resume() async throws {
        try await withCheckedThrowingContinuation {
            (continuation: CheckedContinuation<Void, Error>) in
            virtualMachine.start { result in
                switch result {
                case .success:
                    continuation.resume()
                case .failure(let error):
                    continuation.resume(throwing: error)
                }
            }
        }
    }

    func getVirtualMachine() -> Any {
        return virtualMachine
    }

    // Helper methods for creating common configurations
    static func createStorageDeviceConfiguration(diskPath: Path, readOnly: Bool = false) throws
        -> VZStorageDeviceConfiguration
    {
        return VZVirtioBlockDeviceConfiguration(
            attachment: try VZDiskImageStorageDeviceAttachment(
                url: diskPath.url,
                readOnly: readOnly,
                cachingMode: VZDiskImageCachingMode.automatic,
                synchronizationMode: VZDiskImageSynchronizationMode.fsync
            )
        )
    }

    static func createUSBMassStorageDeviceConfiguration(diskPath: Path, readOnly: Bool = false)
        throws
        -> VZStorageDeviceConfiguration
    {
        if #available(macOS 15.0, *) {
            return VZUSBMassStorageDeviceConfiguration(
                attachment: try VZDiskImageStorageDeviceAttachment(
                    url: diskPath.url,
                    readOnly: readOnly,
                    cachingMode: VZDiskImageCachingMode.automatic,
                    synchronizationMode: VZDiskImageSynchronizationMode.fsync
                )
            )
        } else {
            // Fallback to normal storage device if USB mass storage not available
            return try createStorageDeviceConfiguration(diskPath: diskPath, readOnly: readOnly)
        }
    }

    static func createNetworkDeviceConfiguration(macAddress: String) throws
        -> VZNetworkDeviceConfiguration
    {
        let network = VZVirtioNetworkDeviceConfiguration()
        guard let vzMacAddress = VZMACAddress(string: macAddress) else {
            throw VMConfigError.invalidMachineIdentifier
        }
        network.attachment = VZNATNetworkDeviceAttachment()
        network.macAddress = vzMacAddress
        return network
    }

    static func createDirectorySharingDevices(sharedDirectories: [SharedDirectory]?)
        -> [VZDirectorySharingDeviceConfiguration]
    {
        return sharedDirectories?.map { sharedDir in
            let device = VZVirtioFileSystemDeviceConfiguration(tag: sharedDir.tag)
            let url = URL(fileURLWithPath: sharedDir.hostPath)
            device.share = VZSingleDirectoryShare(
                directory: VZSharedDirectory(url: url, readOnly: sharedDir.readOnly))
            return device
        } ?? []
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

        // Graphics configuration
        // Use host screen-based display configuration when available for better
        // display compositor integration (helps with screenshot capture in VMs)
        let display = VMDisplayResolution(string: config.display)!
        let graphics = VZMacGraphicsDeviceConfiguration()
        if let hostMainScreen = NSScreen.main {
            let vmScreenSize = NSSize(width: display.width, height: display.height)
            graphics.displays = [
                VZMacGraphicsDisplayConfiguration(for: hostMainScreen, sizeInPoints: vmScreenSize)
            ]
        } else {
            // Fallback to pixel-based configuration if no host screen available
            graphics.displays = [
                VZMacGraphicsDisplayConfiguration(
                    widthInPixels: display.width,
                    heightInPixels: display.height,
                    pixelsPerInch: 220
                )
            ]
        }
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
            try createNetworkDeviceConfiguration(macAddress: config.macAddress)
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
        let directorySharingDevices = createDirectorySharingDevices(
            sharedDirectories: config.sharedDirectories)
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
        super.init(
            virtualMachine: VZVirtualMachine(configuration: vzConfig),
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
            try createNetworkDeviceConfiguration(macAddress: config.macAddress)
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
        super.init(virtualMachine: VZVirtualMachine(configuration: vzConfig))
    }
}
