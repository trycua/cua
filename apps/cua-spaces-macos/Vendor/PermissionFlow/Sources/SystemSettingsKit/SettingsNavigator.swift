#if os(macOS)
import AppKit

@available(macOS 13.0, *)
@MainActor
final class SettingsNavigator {
    private let bundleIdentifier = "com.apple.systempreferences"
    private let applicationURL = URL(fileURLWithPath: "/System/Applications/System Settings.app")

    /// Opens System Settings with a generic deeplink URL.
    @discardableResult
    func openSettings(at url: URL) -> Bool {
        NSWorkspace.shared.openApplication(
            at: applicationURL,
            configuration: NSWorkspace.OpenConfiguration()
        ) { @Sendable _, _ in }  // Called off the main thread: not main-actor.

        let didOpen = NSWorkspace.shared.open(url)
        activateSettings()
        return didOpen
    }

    /// Re-activates the running System Settings process if it already exists.
    func activateSettings() {
        NSRunningApplication.runningApplications(withBundleIdentifier: bundleIdentifier)
            .first?
            .activate(options: [.activateIgnoringOtherApps])
    }
}
#elseif os(iOS)
import UIKit

@available(iOS 16.0, *)
@MainActor
final class SettingsNavigator {
    /// Opens the destination URL through UIKit. iOS support is intentionally
    /// limited to URLs that the platform publicly allows.
    @discardableResult
    func openSettings(at url: URL) -> Bool {
        guard UIApplication.shared.canOpenURL(url) else { return false }
        UIApplication.shared.open(url)
        return true
    }
}
#endif
