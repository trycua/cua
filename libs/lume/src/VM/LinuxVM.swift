import Foundation

/// Linux-specific virtual machine implementation
@MainActor
final class LinuxVM: VM {
    override init(
        vmDirContext: VMDirContext,
        virtualizationServiceFactory: @escaping (VMVirtualizationServiceContext) throws -> VMVirtualizationService = { try LinuxVirtualizationService(configuration: $0) },
        vncServiceFactory: @escaping (VMDirectory) -> VNCService = { DefaultVNCService(vmDirectory: $0) },
        displayPresenterFactory: @escaping @MainActor (DisplayMode, VNCService) -> VMDisplayPresenter = defaultDisplayPresenter
    ) {
        super.init(
            vmDirContext: vmDirContext,
            virtualizationServiceFactory: virtualizationServiceFactory,
            vncServiceFactory: vncServiceFactory,
            displayPresenterFactory: displayPresenterFactory
        )
    }

    override func getOSType() -> String {
        return "linux"
    }
    
    override func setup(
        ipswPath: String,
        cpuCount: Int,
        memorySize: UInt64,
        diskSize: UInt64,
        display: String
    ) async throws {

        try setDiskSize(diskSize)

        let service = try virtualizationServiceFactory(
            try createVMVirtualizationServiceContext(
                cpuCount: cpuCount,
                memorySize: memorySize,
                display: display
            )
        )
        guard let linuxService = service as? LinuxVirtualizationService else {
            throw VMError.internalError("Installation requires LinuxVirtualizationService")
        }

        try updateVMConfig(vmConfig: try VMConfig(
            os: getOSType(),
            cpuCount: cpuCount,
            memorySize: memorySize,
            diskSize: diskSize,
            macAddress: linuxService.generateMacAddress(),
            display: display,
            networkMode: vmDirContext.config.networkMode
        ))

        // Create NVRAM store for EFI
        try linuxService.createNVRAM(at: vmDirContext.nvramPath)
    }
} 
