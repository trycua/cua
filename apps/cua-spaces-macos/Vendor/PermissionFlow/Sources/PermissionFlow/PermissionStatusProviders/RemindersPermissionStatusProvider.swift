#if os(macOS)
import EventKit
import Foundation

@available(macOS 13.0, *)
public struct RemindersPermissionStatusProvider: PermissionStatusProviding {
    public var capability: PermissionStatusCapability { .preflightSupported }

    private let provider: CalendarPermissionStatusProvider

    public init() {
        self.provider = CalendarPermissionStatusProvider(entityType: .reminder)
    }

    public func authorizationState() -> PermissionAuthorizationState {
        provider.authorizationState()
    }

    /// Returns whether the process currently has full read/write reminders access.
    public func hasFullAccess() -> Bool {
        provider.hasFullAccess()
    }

    /// Requests reminders access when undetermined, then reports the resulting state.
    ///
    /// Host apps must:
    /// 1. Declare reminders usage description keys in `Info.plist`
    ///    (`NSRemindersUsageDescription`, and on macOS 14+
    ///    `NSRemindersFullAccessUsageDescription`).
    /// 2. If App Sandbox is enabled, grant EventKit personal-data access
    ///    (typically Calendars: `com.apple.security.personal-information.calendars`).
    ///
    /// After a successful first request the app appears in
    /// System Settings > Privacy & Security > Reminders.
    public func requestAuthorization(
        completion: @escaping @Sendable (PermissionAuthorizationState) -> Void
    ) {
        provider.requestAuthorization(completion: completion)
    }
}
#endif
