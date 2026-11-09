#if os(macOS)
import Foundation
import SystemSettingsKit

@available(macOS 13.0, *)
public enum PermissionFlowPane: String, CaseIterable, Codable, Sendable {
    /// App Management permissions list.
    case appManagement
    /// Accessibility permissions list.
    ///
    /// On macOS 27 and later, System Settings labels this page
    /// "Device Control and Data Access". The deeplink remains
    /// `Privacy_Accessibility`.
    case accessibility
    /// Bluetooth permissions list.
    case bluetooth
    /// Calendars permissions list.
    ///
    /// Requires calendar usage description keys in the host `Info.plist`.
    /// Does not use the floating drag panel; opens System Settings only.
    case calendars
    /// Camera permissions list.
    ///
    /// Requires `NSCameraUsageDescription` in the host `Info.plist`.
    /// Does not use the floating drag panel; opens System Settings only.
    case camera
    /// Developer Tools permissions list.
    case developerTools
    /// Full Disk Access permissions list.
    case fullDiskAccess
    /// Input Monitoring permissions list.
    case inputMonitoring
    /// Media & Apple Music permissions list.
    case mediaAppleMusic
    /// Microphone permissions list.
    case microphone
    /// Reminders permissions list.
    ///
    /// Requires reminders usage description keys in the host `Info.plist`.
    /// Does not use the floating drag panel; opens System Settings only.
    case reminders
    /// Screen Recording permissions list.
    case screenRecording

    /// The matching typed Privacy & Security anchor in SystemSettingsKit.
    public var privacyAnchor: PrivacySecurityAnchor {
        switch self {
        case .appManagement: .privacyAppBundles
        case .accessibility: .privacyAccessibility
        case .bluetooth: .privacyBluetooth
        case .calendars: .privacyCalendars
        case .camera: .privacyCamera
        case .developerTools: .privacyDevTools
        case .fullDiskAccess: .privacyAllFiles
        case .inputMonitoring: .privacyListenEvent
        case .mediaAppleMusic: .privacyMedia
        case .microphone: .privacyMicrophone
        case .reminders: .privacyReminders
        case .screenRecording: .privacyScreenCapture
        }
    }

    /// Whether this pane supports the app-list drag authorization panel.
    public var supportsFloatingAuthorizationPanel: Bool {
        switch self {
        case .calendars, .camera, .microphone, .reminders:
            false
        default:
            true
        }
    }

    /// Deep link to the corresponding page inside System Settings.
    public var settingsURL: URL {
        SystemSettingsDestination.privacy(anchor: privacyAnchor).url
    }

    /// Whether System Settings labels the Accessibility privacy page
    /// "Device Control and Data Access".
    ///
    /// This rename landed in macOS 27. The settings URL and
    /// `Privacy_Accessibility` anchor did not change.
    static func usesDeviceControlAndDataAccessTitle(
        operatingSystemVersion: OperatingSystemVersion = ProcessInfo.processInfo.operatingSystemVersion
    ) -> Bool {
        PermissionFlowResources.accessibilityName(operatingSystemVersion: operatingSystemVersion)
            == PermissionFlowResources.deviceControlAndDataAccessNameKey
    }

    /// Returns the localized permission name for the requested locale.
    public func localizedTitle(
        localeIdentifier: String?,
        operatingSystemVersion: OperatingSystemVersion = ProcessInfo.processInfo.operatingSystemVersion
    ) -> String {
        switch self {
        case .appManagement:
            return PermissionFlowLocalizer.string(
                "permission_flow.pane.app_management",
                defaultValue: "App Management",
                localeIdentifier: localeIdentifier
            )
        case .accessibility:
            let nameKey = PermissionFlowResources.accessibilityName(
                operatingSystemVersion: operatingSystemVersion
            )
            let defaultValue = nameKey == PermissionFlowResources.deviceControlAndDataAccessNameKey
                ? "Device Control and Data Access"
                : "Accessibility"
            return PermissionFlowLocalizer.string(
                nameKey,
                defaultValue: defaultValue,
                localeIdentifier: localeIdentifier
            )
        case .bluetooth:
            return PermissionFlowLocalizer.string(
                "permission_flow.pane.bluetooth",
                defaultValue: "Bluetooth",
                localeIdentifier: localeIdentifier
            )
        case .calendars:
            return PermissionFlowLocalizer.string(
                "permission_flow.pane.calendars",
                defaultValue: "Calendars",
                localeIdentifier: localeIdentifier
            )
        case .camera:
            return PermissionFlowLocalizer.string(
                "permission_flow.pane.camera",
                defaultValue: "Camera",
                localeIdentifier: localeIdentifier
            )
        case .developerTools:
            return PermissionFlowLocalizer.string(
                "permission_flow.pane.developer_tools",
                defaultValue: "Developer Tools",
                localeIdentifier: localeIdentifier
            )
        case .fullDiskAccess:
            return PermissionFlowLocalizer.string(
                "permission_flow.pane.full_disk_access",
                defaultValue: "Full Disk Access",
                localeIdentifier: localeIdentifier
            )
        case .inputMonitoring:
            return PermissionFlowLocalizer.string(
                "permission_flow.pane.input_monitoring",
                defaultValue: "Input Monitoring",
                localeIdentifier: localeIdentifier
            )
        case .mediaAppleMusic:
            return PermissionFlowLocalizer.string(
                "permission_flow.pane.media_apple_music",
                defaultValue: "Media & Apple Music",
                localeIdentifier: localeIdentifier
            )
        case .microphone:
            return PermissionFlowLocalizer.string(
                "permission_flow.pane.microphone",
                defaultValue: "Microphone",
                localeIdentifier: localeIdentifier
            )
        case .reminders:
            return PermissionFlowLocalizer.string(
                "permission_flow.pane.reminders",
                defaultValue: "Reminders",
                localeIdentifier: localeIdentifier
            )
        case .screenRecording:
            return PermissionFlowLocalizer.string(
                "permission_flow.pane.screen_recording",
                defaultValue: "Screen Recording",
                localeIdentifier: localeIdentifier
            )
        }
    }
}
#endif
