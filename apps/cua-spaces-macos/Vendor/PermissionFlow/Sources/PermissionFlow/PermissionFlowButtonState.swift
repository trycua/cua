#if os(macOS)
import Foundation

@available(macOS 13.0, *)
public struct PermissionFlowButtonState: Equatable, Sendable {
    /// Localization key for the button title.
    public let titleKey: String
    /// English fallback used when the package resource bundle is unavailable.
    public let defaultTitle: String
    /// SF Symbol name for the button icon.
    public let systemImage: String
    /// Whether the permission is currently granted.
    public let isGranted: Bool

    public init(
        titleKey: String,
        defaultTitle: String,
        systemImage: String,
        isGranted: Bool
    ) {
        self.titleKey = titleKey
        self.defaultTitle = defaultTitle
        self.systemImage = systemImage
        self.isGranted = isGranted
    }

    /// Backward-compatible initializer that derives an English default from
    /// known package keys when the caller does not supply one.
    public init(titleKey: String, systemImage: String, isGranted: Bool) {
        self.titleKey = titleKey
        self.defaultTitle = Self.defaultTitle(for: titleKey)
        self.systemImage = systemImage
        self.isGranted = isGranted
    }

    /// English defaults for built-in button title keys.
    public static func defaultTitle(for titleKey: String) -> String {
        switch titleKey {
        case "permission_flow.button.granted":
            return "Granted"
        case "permission_flow.button.grant":
            return "Grant"
        case "permission_flow.button.open":
            return "Open"
        case "permission_flow.button.checking":
            return "Checking..."
        default:
            return titleKey
        }
    }
}

@available(macOS 13.0, *)
extension PermissionFlowButtonState {
    /// Creates button state from authorization state.
    public static func make(from state: PermissionAuthorizationState) -> Self {
        switch state {
        case .granted:
            .init(
                titleKey: "permission_flow.button.granted",
                defaultTitle: "Granted",
                systemImage: "checkmark.seal.fill",
                isGranted: true
            )
        case .notGranted:
            .init(
                titleKey: "permission_flow.button.grant",
                defaultTitle: "Grant",
                systemImage: "arrow.right.circle.fill",
                isGranted: false
            )
        case .unknown:
            .init(
                titleKey: "permission_flow.button.open",
                defaultTitle: "Open",
                systemImage: "arrow.right.circle.fill",
                isGranted: false
            )
        case .checking:
            .init(
                titleKey: "permission_flow.button.checking",
                defaultTitle: "Checking...",
                systemImage: "clock",
                isGranted: false
            )
        }
    }
}
#endif