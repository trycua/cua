#if os(macOS)
import ApplicationServices
import Foundation

@available(macOS 13.0, *)
public protocol PermissionStatusProviding: Sendable {
    /// Describes whether this provider can reliably check permission status.
    var capability: PermissionStatusCapability { get }
    
    /// Returns the current authorization state for this permission.
    func authorizationState() -> PermissionAuthorizationState
}

@available(macOS 13.0, *)
@MainActor
public enum PermissionStatusRegistry {
    private static let lock = NSLock()
    private static var registeredProviders: [PermissionFlowPane: any PermissionStatusProviding] = [
        .accessibility: AccessibilityPermissionStatusProvider(),
        .calendars: CalendarPermissionStatusProvider(),
        .fullDiskAccess: FullDiskAccessPermissionStatusProvider(),
        .microphone: MicrophonePermissionStatusProvider(),
        .reminders: RemindersPermissionStatusProvider(),
    ]

    /// Registers or replaces a provider for a specific permission pane.
    public static func register(
        provider: any PermissionStatusProviding,
        for pane: PermissionFlowPane
    ) {
        lock.lock()
        defer { lock.unlock() }
        registeredProviders[pane] = provider
    }

    /// Registers multiple providers in one call.
    public static func register(
        providers: [PermissionFlowPane: any PermissionStatusProviding]
    ) {
        lock.lock()
        defer { lock.unlock() }
        for (pane, provider) in providers {
            registeredProviders[pane] = provider
        }
    }

    /// Returns the appropriate status provider for the given permission pane.
    public static func provider(for pane: PermissionFlowPane) -> any PermissionStatusProviding {
        lock.lock()
        defer { lock.unlock() }
        return registeredProviders[pane] ?? UnsupportedPermissionStatusProvider()
    }
}
#endif
