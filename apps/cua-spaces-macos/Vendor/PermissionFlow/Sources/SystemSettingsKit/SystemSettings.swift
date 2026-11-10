import Foundation

@available(macOS 13.0, iOS 16.0, *)
public enum SystemSettings {
#if os(macOS)
    /// Opens a System Settings page from a pane identifier and optional anchor.
    @MainActor
    @discardableResult
    public static func open(
        paneIdentifier: String,
        anchor: String? = nil
    ) -> Bool {
        open(SystemSettingsDestination(paneIdentifier: paneIdentifier, anchor: anchor))
    }

    /// Opens a System Settings page from a prebuilt deeplink destination.
    @MainActor
    @discardableResult
    public static func open(_ destination: SystemSettingsDestination) -> Bool {
        SettingsNavigator().openSettings(at: destination.url)
    }

    /// Opens System Settings from a fully built deeplink URL.
    @MainActor
    @discardableResult
    public static func open(url: URL) -> Bool {
        SettingsNavigator().openSettings(at: url)
    }

    /// Re-activates the running System Settings app if it already exists.
    @MainActor
    public static func activate() {
        SettingsNavigator().activateSettings()
    }
#elseif os(iOS)
    /// Opens the current app's Settings page on iOS.
    @MainActor
    @discardableResult
    public static func openAppSettings() -> Bool {
        SettingsNavigator().openSettings(at: SystemSettingsDestination.appSettings.url)
    }

    /// Opens the current app's notification settings page on iOS.
    @MainActor
    @discardableResult
    public static func openNotificationSettings() -> Bool {
        SettingsNavigator().openSettings(at: SystemSettingsDestination.notificationSettings.url)
    }

    /// Opens a Settings page from a prebuilt destination. iOS only exposes the
    /// destinations explicitly modelled in `SystemSettingsDestination`.
    @MainActor
    @discardableResult
    public static func open(_ destination: SystemSettingsDestination) -> Bool {
        SettingsNavigator().openSettings(at: destination.url)
    }

    /// Opens Settings from a fully built URL. The URL must be supported by iOS.
    @MainActor
    @discardableResult
    public static func open(url: URL) -> Bool {
        SettingsNavigator().openSettings(at: url)
    }
#endif
}
