import Foundation

/// Errors that can occur during SSH operations
public enum SSHError: Error, LocalizedError {
    case vmNotFound(String)
    case vmNotRunning(String)
    case noIPAddress(String)
    case sshNotAvailable(String)
    case connectionFailed(String)
    case authenticationFailed
    case commandFailed(exitCode: Int32, message: String)
    case timeout
    case ptyCreationFailed
    case forkFailed
    case execFailed

    public var errorDescription: String? {
        switch self {
        case .vmNotFound(let name):
            return "VM '\(name)' not found"
        case .vmNotRunning(let name):
            return "VM '\(name)' is not running"
        case .noIPAddress(let name):
            return "VM '\(name)' has no IP address. Wait for it to boot completely."
        case .sshNotAvailable(let name):
            return "SSH is not available on VM '\(name)'. Ensure SSH/Remote Login is enabled."
        case .connectionFailed(let message):
            return "SSH connection failed: \(message)"
        case .authenticationFailed:
            return "SSH authentication failed. Check username and password."
        case .commandFailed(let exitCode, let message):
            return "Command failed with exit code \(exitCode): \(message)"
        case .timeout:
            return "SSH operation timed out"
        case .ptyCreationFailed:
            return "Failed to create pseudo-terminal"
        case .forkFailed:
            return "Failed to fork process"
        case .execFailed:
            return "Failed to execute SSH"
        }
    }
}
