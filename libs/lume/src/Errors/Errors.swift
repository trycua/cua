import Foundation

enum HomeError: Error, LocalizedError {
    case directoryCreationFailed(path: String)
    case directoryAccessDenied(path: String)
    case invalidHomeDirectory
    case directoryAlreadyExists(path: String)
    case homeNotFound
    case defaultStorageNotDefined
    case storageLocationNotFound(String)
    case storageLocationNotADirectory(String)
    case storageLocationNotWritable(String)
    case invalidStorageLocation(String)
    case cannotCreateDirectory(String)
    case cannotGetVMsDirectory
    case vmDirectoryNotFound(String)
    
    var errorDescription: String? {
        switch self {
        case .directoryCreationFailed(let path):
            return "Failed to create directory at path: \(path)"
        case .directoryAccessDenied(let path):
            return "Access denied to directory at path: \(path)"
        case .invalidHomeDirectory:
            return "Invalid home directory configuration"
        case .directoryAlreadyExists(let path):
            return "Directory already exists at path: \(path)"
        case .homeNotFound:
            return "Home directory not found."
        case .defaultStorageNotDefined:
            return "Default storage location is not defined."
        case .storageLocationNotFound(let path):
            return "Storage location not found: \(path)"
        case .storageLocationNotADirectory(let path):
            return "Storage location is not a directory: \(path)"
        case .storageLocationNotWritable(let path):
            return "Storage location is not writable: \(path)"
        case .invalidStorageLocation(let path):
            return "Invalid storage location specified: \(path)"
        case .cannotCreateDirectory(let path):
            return "Cannot create directory: \(path)"
        case .cannotGetVMsDirectory:
            return "Cannot determine the VMs directory."
        case .vmDirectoryNotFound(let path):
            return "VM directory not found: \(path)"
        }
    }
}

enum PullError: Error, LocalizedError {
    case invalidImageFormat
    case tokenFetchFailed
    case manifestFetchFailed
    case layerDownloadFailed(String)
    case missingPart(Int)
    case decompressionFailed(String)
    case reassemblyFailed(String)
    case fileCreationFailed(String)
    case reassemblySetupFailed(path: String, underlyingError: Error)
    case missingUncompressedSizeAnnotation
    
    var errorDescription: String? {
        switch self {
        case .invalidImageFormat:
            return "Invalid image format. Expected format: name:tag"
        case .tokenFetchFailed:
            return "Failed to fetch authentication token from registry."
        case .manifestFetchFailed:
            return "Failed to fetch image manifest from registry."
        case .layerDownloadFailed(let digest):
            return "Failed to download layer: \(digest)"
        case .missingPart(let partNum):
            return "Missing required part number \(partNum) for reassembly."
        case .decompressionFailed(let file):
            return "Failed to decompress file: \(file)"
        case .reassemblyFailed(let reason):
            return "Disk image reassembly failed: \(reason)."
        case .fileCreationFailed(let path):
            return "Failed to create the necessary file at path: \(path)"
        case .reassemblySetupFailed(let path, let underlyingError):
            return "Failed to set up for reassembly at path: \(path). Underlying error: \(underlyingError.localizedDescription)"
        case .missingUncompressedSizeAnnotation:
            return "Could not find the required uncompressed disk size annotation in the image config.json."
        }
    }
}

enum VMConfigError: CustomNSError, LocalizedError {
    case invalidDisplayResolution(String)
    case invalidMachineIdentifier
    case emptyMachineIdentifier
    case emptyHardwareModel
    case invalidHardwareModel
    case invalidDiskSize
    case malformedSizeInput(String)
    
    var errorDescription: String? {
        switch self {
        case .invalidDisplayResolution(let resolution):
            return "Invalid display resolution: \(resolution)"
        case .emptyMachineIdentifier:
            return "Empty machine identifier"
        case .invalidMachineIdentifier:
            return "Invalid machine identifier"
        case .emptyHardwareModel:
            return "Empty hardware model"
        case .invalidHardwareModel:
            return "Invalid hardware model: the host does not support the hardware model"
        case .invalidDiskSize:
            return "Invalid disk size"
        case .malformedSizeInput(let input):
            return "Malformed size input: \(input)"
        }
    }
    
    static var errorDomain: String { "VMConfigError" }
    
    var errorCode: Int {
        switch self {
        case .invalidDisplayResolution: return 1
        case .emptyMachineIdentifier: return 2
        case .invalidMachineIdentifier: return 3
        case .emptyHardwareModel: return 4
        case .invalidHardwareModel: return 5
        case .invalidDiskSize: return 6
        case .malformedSizeInput: return 7
        }
    }
}

enum VMDirectoryError: Error, LocalizedError {
    case configNotFound
    case invalidConfigData
    case diskOperationFailed(String)
    case fileCreationFailed(String)
    case sessionNotFound
    case invalidSessionData
    
    var errorDescription: String {
        switch self {
        case .configNotFound:
            return "VM configuration file not found"
        case .invalidConfigData:
            return "Invalid VM configuration data"
        case .diskOperationFailed(let reason):
            return "Disk operation failed: \(reason)"
        case .fileCreationFailed(let path):
            return "Failed to create file at path: \(path)"
        case .sessionNotFound:
            return "VNC session file not found"
        case .invalidSessionData:
            return "Invalid VNC session data"
        }
    }
}

enum VMError: Error, LocalizedError {
    case alreadyExists(String)
    case notFound(String)
    case notInitialized(String)
    case notRunning(String)
    case alreadyRunning(String)
    case installNotStarted(String)
    case stopTimeout(String)
    case resizeTooSmall(current: UInt64, requested: UInt64)
    case vncNotConfigured
    case vncPortBindingFailed(requested: Int, actual: Int)
    case internalError(String)
    case unsupportedOS(String)
    case invalidDisplayResolution(String)
    var errorDescription: String? {
        switch self {
        case .alreadyExists(let name):
            return "Virtual machine already exists with name: \(name)"
        case .notFound(let name):
            return "Virtual machine not found: \(name)"
        case .notInitialized(let name):
            return "Virtual machine not initialized: \(name)"
        case .notRunning(let name):
            return "Virtual machine not running: \(name)"
        case .alreadyRunning(let name):
            return "Virtual machine already running: \(name)"
        case .installNotStarted(let name):
            return "Virtual machine install not started: \(name)"
        case .stopTimeout(let name):
            return "Timeout while stopping virtual machine: \(name)"
        case .resizeTooSmall(let current, let requested):
            return "Cannot resize disk to \(requested) bytes, current size is \(current) bytes"
        case .vncNotConfigured:
            return "VNC is not configured for this virtual machine"
        case .vncPortBindingFailed(let requested, let actual):
            if actual == -1 {
                return "Could not bind to VNC port \(requested) (port already in use). Try a different port or use port 0 for auto-assign."
            }
            return "Could not bind to VNC port \(requested) (port already in use). System assigned port \(actual) instead. Try a different port or use port 0 for auto-assign."
        case .internalError(let message):
            return "Internal error: \(message)"
        case .unsupportedOS(let os):
            return "Unsupported operating system: \(os)"
        case .invalidDisplayResolution(let resolution):
            return "Invalid display resolution: \(resolution)"
        }
    }
}

enum ResticError: Error {
    case snapshotFailed(String)
    case restoreFailed(String)
    case genericError(String)
}

enum VmrunError: Error, LocalizedError {
    case commandNotFound
    case operationFailed(command: String, output: String?)

    var errorDescription: String? {
        switch self {
        case .commandNotFound:
            return "vmrun command not found. Ensure VMware Fusion is installed and in the system PATH."
        case .operationFailed(let command, let output):
            return "vmrun command '\(command)' failed. Output: \(output ?? "No output")"
        }
    }
}