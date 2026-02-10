import Foundation

/// macOS-specific virtual machine implementation
@MainActor
final class DarwinVM: VM {
    private let imageLoader: ImageLoader

    init(
        vmDirContext: VMDirContext,
        virtualizationServiceFactory: @escaping (VMVirtualizationServiceContext) throws -> VMVirtualizationService = { try DarwinVirtualizationService(configuration: $0) },
        vncServiceFactory: @escaping (VMDirectory) -> VNCService = { DefaultVNCService(vmDirectory: $0) },
        imageLoader: ImageLoader
    ) {
        self.imageLoader = imageLoader
        super.init(
            vmDirContext: vmDirContext,
            virtualizationServiceFactory: virtualizationServiceFactory,
            vncServiceFactory: vncServiceFactory
        )
    }

    override func getOSType() -> String {
        return "macOS"
    }

    // MARK: - Installation and Configuration
    
    override func setup(ipswPath: String, cpuCount: Int, memorySize: UInt64, diskSize: UInt64, display: String) async throws {
        let imagePath: Path
        if ipswPath == "latest" {
            Logger.info("Downloading latest supported Image...")
            let downloadedPath = try await self.imageLoader.downloadLatestImage()
            imagePath = Path(downloadedPath.path)
        } else {
            imagePath = Path(ipswPath)
        }

        let requirements = try await imageLoader.loadImageRequirements(from: imagePath.url)
        try setDiskSize(diskSize)

        let finalCpuCount = max(cpuCount, requirements.minimumSupportedCPUCount)
        try setCpuCount(finalCpuCount)
        if finalCpuCount != cpuCount {
            Logger.info("CPU count overridden due to minimum image requirements", metadata: ["original": "\(cpuCount)", "final": "\(finalCpuCount)"])
        }

        let finalMemorySize = max(memorySize, requirements.minimumSupportedMemorySize)
        try setMemorySize(finalMemorySize)
        if finalMemorySize != memorySize {
            Logger.info("Memory size overridden due to minimum image requirements", metadata: ["original": "\(memorySize)", "final": "\(finalMemorySize)"])
        }

        try updateVMConfig(
            vmConfig: try VMConfig(
                os: getOSType(),
                cpuCount: finalCpuCount,
                memorySize: finalMemorySize,
                diskSize: diskSize,
                macAddress: DarwinVirtualizationService.generateMacAddress(),
                display: display,
                hardwareModel: requirements.hardwareModel,
                machineIdentifier: DarwinVirtualizationService.generateMachineIdentifier(),
                networkMode: vmDirContext.config.networkMode
            )
        )

        let service: any VMVirtualizationService = try virtualizationServiceFactory(
            try createVMVirtualizationServiceContext(
                cpuCount: finalCpuCount,
                memorySize: finalMemorySize,
                display: display
            )
        )
        guard let darwinService = service as? DarwinVirtualizationService else {
            throw VMError.internalError("Installation requires DarwinVirtualizationService")
        }

        // Create auxiliary storage with hardware model
        try darwinService.createAuxiliaryStorage(at: vmDirContext.nvramPath, hardwareModel: requirements.hardwareModel)

        try await darwinService.installMacOS(imagePath: imagePath) { progress in
            Logger.info("Installing macOS", metadata: ["progress": "\(Int(progress * 100))%"])
        }
    }
}
