import Foundation
import Virtualization

enum VMType: String {
    case darwin = "macOS"
    case linux = "linux"
}

protocol VMFactory {
    @MainActor
    func createVM(
        vmDirContext: VMDirContext,
        imageLoader: ImageLoader?
    ) throws -> VM
}

class DefaultVMFactory: VMFactory {
    @MainActor
    func createVM(
        vmDirContext: VMDirContext,
        imageLoader: ImageLoader?
    ) throws -> VM {
        let osType = vmDirContext.config.os.lowercased()
        
        switch osType {
        case "macos", "darwin":
            guard let imageLoader = imageLoader else {
                throw VMError.internalError("ImageLoader required for macOS VM")
            }
            return DarwinVM(vmDirContext: vmDirContext, imageLoader: imageLoader)
        case "linux":
            return LinuxVM(vmDirContext: vmDirContext)
        default:
            throw VMError.unsupportedOS(osType)
        }
    }
}