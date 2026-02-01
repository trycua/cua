import Foundation

/// Errors that can occur during clipboard operations
public enum ClipboardError: Error, LocalizedError {
    case contentTooLarge(Int, Int)
    case unsupportedContentType
    case vmCommandFailed(String)
    case decodingFailed
    case hostClipboardEmpty
    case vmClipboardEmpty
    case hostClipboardReadFailed

    public var errorDescription: String? {
        switch self {
        case .contentTooLarge(let actual, let max):
            return "Clipboard content too large: \(actual) bytes (max: \(max) bytes)"
        case .unsupportedContentType:
            return "Unsupported clipboard content type. Only text is currently supported."
        case .vmCommandFailed(let output):
            return "VM clipboard command failed: \(output)"
        case .decodingFailed:
            return "Failed to decode clipboard content from VM"
        case .hostClipboardEmpty:
            return "Host clipboard is empty"
        case .vmClipboardEmpty:
            return "VM clipboard is empty"
        case .hostClipboardReadFailed:
            return "Failed to read host clipboard"
        }
    }
}
