import Foundation
#if os(iOS)
import UIKit
#endif

#if os(macOS)
/// Anchor points for navigating to specific sections within the Displays settings pane.
/// These anchors allow direct navigation to subsections of the macOS System Settings Displays panel.
@available(macOS 13.0, *)
public enum DisplaySettingsAnchor: String, CaseIterable, Sendable {
    /// Opens the Advanced section in Displays settings.
    case advancedSection
    /// Opens ambient light and True Tone related settings.
    case ambienceSection
    /// Opens display arrangement and positioning controls.
    case arrangementSection
    /// Opens display characteristics and calibration options.
    case characteristicSection
    /// Opens the main Displays section.
    case displaysSection
    /// Opens miscellaneous display settings.
    case miscellaneousSection
    /// Opens Night Shift settings.
    case nightShiftSection
    /// Opens color profile settings.
    case profileSection
    /// Opens resolution settings.
    case resolutionSection
    /// Opens Sidecar settings.
    case sidecarSection
}

/// Anchor points for navigating to specific sections within the Login Items settings pane.
/// These anchors allow direct navigation to login items and extension items.
@available(macOS 13.0, *)
public enum LoginItemsSettingsAnchor: String, CaseIterable, Sendable {
    /// Opens the extension items section.
    case extensionItems = "ExtensionItems"
}

/// Extension point identifiers that System Settings can open inside Login Items & Extensions.
@available(macOS 13.0, *)
public enum LoginItemsExtensionPointIdentifier: String, CaseIterable, Sendable {
    /// Opens the Share Menu extension settings.
    case shareServices = "com.apple.share-services"
    /// Opens the Actions extension settings.
    case actions = "com.apple.ui-services"
    /// Opens the Photo Editing extension settings.
    case photoEditing = "com.apple.photo-editing"
    /// Opens the Spotlight importer extension settings.
    case spotlightImporter = "com.apple.spotlight.import"
    /// Opens the Quick Look previews extension settings.
    case quickLookPreview = "com.apple.quicklook.preview"
    /// Opens the File Provider extension settings.
    case fileProvider = "com.apple.fileprovider-nonui"
    /// Opens the Finder Quick Actions extension settings.
    case finderQuickActions = "com.apple.finder-quick-actions"
    /// Opens the Touch Bar Quick Actions extension settings.
    case touchBarQuickActions = "com.apple.touchbar-quick-actions"
    /// Opens the legacy Dock tile plugin settings.
    case legacyDockTiles = "com.apple.extensionkit.legacy-plugins.docktiles"
    /// Opens the legacy Spotlight importer plugin settings.
    case legacySpotlightImporter = "com.apple.extensionkit.legacy-plugins.spotlight-importer"
}

/// Anchor points for navigating to specific sections within the Wi-Fi settings pane.
/// These anchors allow direct navigation to Wi-Fi overview, join, details, and advanced sections.
@available(macOS 13.0, *)
public enum WiFiSettingsAnchor: String, CaseIterable, Sendable {
    /// Opens advanced Wi-Fi settings.
    case advanced = "Advanced"
    /// Opens Wi-Fi network details.
    case generalDetails = "General_Details"
    /// Opens the join network section.
    case generalJoin = "General_Join"
    /// Opens the main Wi-Fi section.
    case generalMain = "General_Main"
}

/// Anchor points for navigating to specific sections within the VPN settings pane.
/// These anchors allow direct navigation to VPN and VPN on Demand settings.
@available(macOS 13.0, *)
public enum VPNSettingsAnchor: String, CaseIterable, Sendable {
    /// Opens VPN settings.
    case vpn = "VPN"
    /// Opens VPN on Demand settings.
    case vpnOnDemand = "VPN on Demand"
}

/// Common anchor points for navigating to major sections within the Accessibility settings pane.
/// Use `SystemSettingsDestination.accessibility(anchor: String)` for less common control-level anchors.
@available(macOS 13.0, *)
public enum AccessibilitySettingsAnchor: String, CaseIterable, Sendable {
    /// Opens the Display accessibility section.
    case display = "display"
    /// Opens the Text accessibility section.
    case text = "text"
    /// Opens pointer appearance settings.
    case pointer = "pointer"
    /// Opens mouse and trackpad accessibility settings.
    case mouseAndTrackpad = "mouseAndTrackpad"
    /// Opens headphone accessibility settings.
    case headphones = "headphones"
    /// Opens VoiceOver settings.
    case voiceOver = "AX_feature.voiceOver"
    /// Opens Zoom accessibility settings.
    case zoom = "AX_feature.zoom"
    /// Opens display filter settings.
    case displayFilters = "AX_feature.displayFilters"
    /// Opens background sounds settings.
    case backgroundSounds = "AX_feature.backgroundSounds"
    /// Opens Spoken Content settings.
    case spokenContent = "AX_FEATURE_SPOKENCONTENT"
    /// Opens Captions settings.
    case captions = "AX_FEATURE_CAPTIONS"
    /// Opens Audio accessibility settings.
    case audio = "AX_FEATURE_AUDIO"
    /// Opens Audio Descriptions settings.
    case descriptions = "AX_FEATURE_DESCRIPTIONS"
    /// Opens Keyboard accessibility settings.
    case keyboard = "AX_FEATURE_KEYBOARD"
    /// Opens Full Keyboard Access settings.
    case fullKeyboardAccess = "AX_feature.fullKeyboardAccess"
    /// Opens Sticky Keys settings.
    case stickyKeys = "AX_feature.stickyKeys"
    /// Opens Slow Keys settings.
    case slowKeys = "AX_feature.slowKeys"
    /// Opens Accessibility Keyboard settings.
    case virtualKeyboard = "AX_feature.virtualKeyboard"
    /// Opens Voice Control settings.
    case voiceControl = "AX_feature.voiceControl"
    /// Opens Switch Control settings.
    case switchControl = "AX_feature.switchControl"
    /// Opens Alternate Mouse Buttons settings.
    case alternateMouseButtons = "AX_feature.alternateMouseButtons"
    /// Opens Head Pointer settings.
    case headMouse = "AX_feature.headMouse"
    /// Opens Mouse Keys settings.
    case mouseKeys = "AX_feature.mouseKeys"
    /// Opens Hover Text settings.
    case hoverText = "AX_feature.hoverText"
    /// Opens Hover Typing settings.
    case hoverTyping = "AX_feature.hoverTyping"
    /// Opens Live Speech settings.
    case liveSpeech = "AX_feature.liveSpeech"
    /// Opens Personal Voice settings.
    case personalVoice = "AX_FEATURE_PERSONALVOICE"
    /// Opens Siri accessibility settings.
    case siri = "AX_FEATURE_SIRI"
    /// Opens Accessibility Shortcut settings.
    case shortcut = "AX_FEATURE_SHORTCUT"
}

/// Anchor points for navigating to specific sections within the Privacy & Security settings pane.
/// These anchors allow direct navigation to subsections of the macOS System Settings Privacy & Security panel.
@available(macOS 13.0, *)
public enum PrivacySecurityAnchor: String, CaseIterable, Sendable {
    /// Opens the Advanced section in Privacy & Security.
    case advanced = "Advanced"
    /// Opens FileVault settings.
    case fileVault = "FileVault"
    /// Opens the Location Access Report section.
    case locationAccessReport = "Location_Access_Report"
    /// Opens Lockdown Mode settings.
    case lockdownMode = "LockdownMode"
    /// Opens Accessibility privacy permissions.
    ///
    /// The System Settings list label is "Accessibility" before macOS 27
    /// and "Device Control and Data Access" on macOS 27 and later.
    /// The `Privacy_Accessibility` anchor is unchanged.
    case privacyAccessibility = "Privacy_Accessibility"
    /// Opens Advertising privacy settings.
    case privacyAdvertising = "Privacy_Advertising"
    /// Opens Full Disk Access permissions.
    case privacyAllFiles = "Privacy_AllFiles"
    /// Opens Analytics privacy settings.
    case privacyAnalytics = "Privacy_Analytics"
    /// Opens app bundle access permissions.
    case privacyAppBundles = "Privacy_AppBundles"
    /// Opens audio capture permissions.
    case privacyAudioCapture = "Privacy_AudioCapture"
    /// Opens Automation permissions.
    case privacyAutomation = "Privacy_Automation"
    /// Opens Bluetooth permissions.
    case privacyBluetooth = "Privacy_Bluetooth"
    /// Opens Calendar permissions.
    case privacyCalendars = "Privacy_Calendars"
    /// Opens Camera permissions.
    case privacyCamera = "Privacy_Camera"
    /// Opens Contacts permissions.
    case privacyContacts = "Privacy_Contacts"
    /// Opens Developer Tools permissions.
    case privacyDevTools = "Privacy_DevTools"
    /// Opens Files and Folders permissions.
    case privacyFilesAndFolders = "Privacy_FilesAndFolders"
    /// Opens Focus permissions.
    case privacyFocus = "Privacy_Focus"
    /// Opens HomeKit permissions.
    case privacyHomeKit = "Privacy_HomeKit"
    /// Opens input and event monitoring permissions.
    case privacyListenEvent = "Privacy_ListenEvent"
    /// Opens Location Services permissions.
    case privacyLocationServices = "Privacy_LocationServices"
    /// Opens Media and Apple Music permissions.
    case privacyMedia = "Privacy_Media"
    /// Opens Microphone permissions.
    case privacyMicrophone = "Privacy_Microphone"
    /// Opens Motion and Fitness permissions.
    case privacyMotion = "Privacy_Motion"
    /// Opens nudity detection privacy settings.
    case privacyNudityDetection = "Privacy_NudityDetection"
    /// Opens passkey access permissions.
    case privacyPasskeyAccess = "Privacy_PasskeyAccess"
    /// Opens Photos permissions.
    case privacyPhotos = "Privacy_Photos"
    /// Opens Reminders permissions.
    case privacyReminders = "Privacy_Reminders"
    /// Opens Remote Desktop permissions.
    case privacyRemoteDesktop = "Privacy_RemoteDesktop"
    /// Opens Screen Recording permissions.
    case privacyScreenCapture = "Privacy_ScreenCapture"
    /// Opens Speech Recognition permissions.
    case privacySpeechRecognition = "Privacy_SpeechRecognition"
    /// Opens System Services privacy settings.
    case privacySystemServices = "Privacy_SystemServices"
    /// Opens the main Security section.
    case security = "Security"
    /// Opens Security Improvements recommendations.
    case securityImprovements = "SecurityImprovements"
}
#endif

@available(macOS 13.0, iOS 16.0, *)
public struct SystemSettingsDestination: Hashable, Sendable {
    public let url: URL

    /// The pane or extension identifier used by System Settings when the
    /// destination is backed by a macOS deeplink.
    public let paneIdentifier: String?

    /// Optional anchor for a subsection inside a macOS pane.
    public let anchor: String?

    public init(url: URL, paneIdentifier: String? = nil, anchor: String? = nil) {
        self.url = url
        self.paneIdentifier = paneIdentifier
        self.anchor = anchor
    }
}

#if os(macOS)
@available(macOS 13.0, *)
public extension SystemSettingsDestination {
    init(paneIdentifier: String, anchor: String? = nil) {
        let encodedAnchor = anchor?.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed)
        let value = if let encodedAnchor, encodedAnchor.isEmpty == false {
            "x-apple.systempreferences:\(paneIdentifier)?\(encodedAnchor)"
        } else {
            "x-apple.systempreferences:\(paneIdentifier)"
        }

        self.init(
            url: URL(string: value)!,
            paneIdentifier: paneIdentifier,
            anchor: anchor
        )
    }
}

@available(macOS 13.0, *)
public extension SystemSettingsDestination {
    /// Privacy & Security home page.
    static func privacy() -> Self {
        Self(paneIdentifier: "com.apple.settings.PrivacySecurity.extension")
    }

    /// Convenience helper for the Privacy & Security extension anchors.
    /// Example anchors include `Privacy_Advertising` and `Privacy_AllFiles`.
    static func privacy(anchor: String) -> Self {
        Self(
            paneIdentifier: "com.apple.settings.PrivacySecurity.extension",
            anchor: anchor
        )
    }

    /// Convenience helper for typed Privacy & Security anchors.
    static func privacy(anchor: PrivacySecurityAnchor) -> Self {
        Self(
            paneIdentifier: "com.apple.settings.PrivacySecurity.extension",
            anchor: anchor.rawValue
        )
    }

    /// Wallpaper settings.
    static let wallpaper = Self(paneIdentifier: "com.apple.Wallpaper-Settings.extension")

    /// Displays settings.
    static let displays = Self(paneIdentifier: "com.apple.Displays-Settings.extension")

    /// Displays settings subsection.
    static func displays(anchor: DisplaySettingsAnchor) -> Self {
        Self(
            paneIdentifier: "com.apple.Displays-Settings.extension",
            anchor: anchor.rawValue
        )
    }

    /// Bluetooth settings.
    static let bluetooth = Self(paneIdentifier: "com.apple.BluetoothSettings")

    /// Login items settings.
    static let loginItems = Self(paneIdentifier: "com.apple.LoginItems-Settings.extension")

    /// Login items settings subsection.
    static func loginItems(anchor: LoginItemsSettingsAnchor) -> Self {
        Self(
            paneIdentifier: "com.apple.LoginItems-Settings.extension",
            anchor: anchor.rawValue
        )
    }

    /// Login Items & Extensions settings for a specific extension point identifier.
    static func loginItems(extensionPointIdentifier: String) -> Self {
        Self(
            paneIdentifier: "com.apple.ExtensionsPreferences",
            anchor: "extensionPointIdentifier=\(extensionPointIdentifier)"
        )
    }

    /// Login Items & Extensions settings for a typed extension point identifier.
    static func loginItems(extensionPointIdentifier: LoginItemsExtensionPointIdentifier) -> Self {
        loginItems(extensionPointIdentifier: extensionPointIdentifier.rawValue)
    }

    /// Wi-Fi settings.
    static let wifi = Self(paneIdentifier: "com.apple.wifi-settings-extension")

    /// Wi-Fi settings subsection.
    static func wifi(anchor: WiFiSettingsAnchor) -> Self {
        Self(
            paneIdentifier: "com.apple.wifi-settings-extension",
            anchor: anchor.rawValue
        )
    }

    /// VPN settings.
    static let vpn = Self(paneIdentifier: "com.apple.NetworkExtensionSettingsUI.NESettingsUIExtension*vpn")

    /// VPN settings subsection.
    static func vpn(anchor: VPNSettingsAnchor) -> Self {
        Self(
            paneIdentifier: "com.apple.NetworkExtensionSettingsUI.NESettingsUIExtension*vpn",
            anchor: anchor.rawValue
        )
    }

    /// Accessibility settings.
    static let accessibility = Self(paneIdentifier: "com.apple.Accessibility-Settings.extension")

    /// Accessibility settings subsection.
    static func accessibility(anchor: AccessibilitySettingsAnchor) -> Self {
        Self(
            paneIdentifier: "com.apple.Accessibility-Settings.extension",
            anchor: anchor.rawValue
        )
    }

    /// Accessibility settings subsection from a raw anchor string.
    static func accessibility(anchor: String) -> Self {
        Self(
            paneIdentifier: "com.apple.Accessibility-Settings.extension",
            anchor: anchor
        )
    }
}
#elseif os(iOS)
@available(iOS 16.0, *)
public extension SystemSettingsDestination {
    /// Opens the current app's Settings screen on iOS.
    static let appSettings = Self(url: URL(string: UIApplication.openSettingsURLString)!)

    /// Opens the current app's notification settings screen on iOS.
    static let notificationSettings = Self(url: URL(string: UIApplication.openNotificationSettingsURLString)!)
}
#endif
