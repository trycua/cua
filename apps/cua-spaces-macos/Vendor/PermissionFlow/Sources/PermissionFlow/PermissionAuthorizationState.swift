#if os(macOS)
import Foundation

@available(macOS 13.0, *)
public enum PermissionAuthorizationState: String, CaseIterable, Codable, Equatable, Sendable {
    /// Permission is granted and ready to use.
    case granted
    /// Permission is explicitly not granted.
    case notGranted
    /// Permission status cannot be determined reliably.
    case unknown
    /// Permission status check is in progress.
    case checking
}
#endif