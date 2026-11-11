#if os(macOS)
import Foundation

@available(macOS 13.0, *)
public struct UnsupportedPermissionStatusProvider: PermissionStatusProviding {
    public var capability: PermissionStatusCapability { .unsupported }
    
    public func authorizationState() -> PermissionAuthorizationState {
        // For permissions that don't have reliable programmatic checking,
        // we always return unknown to indicate the user needs to check manually
        return .unknown
    }
    
    public init() {}
}
#endif