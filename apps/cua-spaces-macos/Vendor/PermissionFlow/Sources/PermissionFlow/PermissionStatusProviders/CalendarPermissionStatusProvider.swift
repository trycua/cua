#if os(macOS)
import EventKit
import Foundation

@available(macOS 13.0, *)
public struct CalendarPermissionStatusProvider: PermissionStatusProviding {
    public var capability: PermissionStatusCapability { .preflightSupported }

    /// Entity type used for calendar (event) permission checks.
    public let entityType: EKEntityType

    public init(entityType: EKEntityType = .event) {
        self.entityType = entityType
    }

    public func authorizationState() -> PermissionAuthorizationState {
        Self.authorizationState(for: EKEventStore.authorizationStatus(for: entityType))
    }

    /// Returns whether the process currently has full read/write calendar access.
    public func hasFullAccess() -> Bool {
        authorizationState() == .granted
    }

    /// Requests calendar access when undetermined, then reports the resulting state.
    ///
    /// Host apps must:
    /// 1. Declare calendar usage description keys in `Info.plist`
    ///    (`NSCalendarsUsageDescription`, and on macOS 14+
    ///    `NSCalendarsFullAccessUsageDescription`).
    /// 2. If App Sandbox is enabled, turn on Calendars access
    ///    (`com.apple.security.personal-information.calendars` /
    ///    `ENABLE_RESOURCE_ACCESS_CALENDARS = YES`).
    ///
    /// After a successful first request the app appears in
    /// System Settings > Privacy & Security > Calendars.
    public func requestAuthorization(
        completion: @escaping @Sendable (PermissionAuthorizationState) -> Void
    ) {
        let status = EKEventStore.authorizationStatus(for: entityType)
        switch status {
        case .notDetermined:
            requestAccess(completion: completion)
        default:
            completion(Self.authorizationState(for: status))
        }
    }

    private func requestAccess(
        completion: @escaping @Sendable (PermissionAuthorizationState) -> Void
    ) {
        // Keep the store alive until the async callback returns. A stack-local
        // EKEventStore can be released before TCC finishes registering the app.
        let store = CalendarEventStoreBox.shared.store(for: entityType)
        if #available(macOS 14.0, *) {
            switch entityType {
            case .event:
                store.requestFullAccessToEvents { _, _ in
                    completion(self.authorizationState())
                }
            case .reminder:
                store.requestFullAccessToReminders { _, _ in
                    completion(self.authorizationState())
                }
            @unknown default:
                completion(.unknown)
            }
        } else {
            store.requestAccess(to: entityType) { _, _ in
                completion(self.authorizationState())
            }
        }
    }

    private static func authorizationState(
        for status: EKAuthorizationStatus
    ) -> PermissionAuthorizationState {
        switch status {
        case .fullAccess:
            // macOS 14+: full read/write calendar access.
            return .granted
        case .authorized:
            // Pre-macOS 14 (and deprecated alias on newer SDKs).
            return .granted
        case .writeOnly, .denied, .restricted, .notDetermined:
            // writeOnly is not treated as full calendar read access.
            return .notGranted
        @unknown default:
            return .unknown
        }
    }
}

/// Holds a long-lived `EKEventStore` so permission requests are not cancelled
/// when a temporary store is deallocated (Apple recommends reusing one store).
@available(macOS 13.0, *)
private final class CalendarEventStoreBox: @unchecked Sendable {
    static let shared = CalendarEventStoreBox()

    private let lock = NSLock()
    private var sharedStore: EKEventStore?

    func store(for entityType: EKEntityType) -> EKEventStore {
        // A single process-wide store is enough for both events and reminders
        // permission prompts; Apple recommends reusing one event store.
        _ = entityType
        lock.lock()
        defer { lock.unlock() }
        if let sharedStore {
            return sharedStore
        }
        let created = EKEventStore()
        sharedStore = created
        return created
    }
}
#endif
