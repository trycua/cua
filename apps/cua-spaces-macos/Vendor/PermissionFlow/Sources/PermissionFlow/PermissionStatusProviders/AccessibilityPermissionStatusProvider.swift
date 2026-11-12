#if os(macOS)
import ApplicationServices
import Foundation

@available(macOS 13.0, *)
public struct AccessibilityPermissionStatusProvider: PermissionStatusProviding {
    public var capability: PermissionStatusCapability { .preflightSupported }
    
    public func authorizationState() -> PermissionAuthorizationState {
        // Check if accessibility is enabled for the current process
        let isEnabled = AXIsProcessTrusted()
        return isEnabled ? .granted : .notGranted
    }
    
    public init() {}
}
#endif