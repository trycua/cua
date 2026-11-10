import Foundation

/// Public access to the package resource bundle with packaged-app fallbacks.
///
/// SwiftPM's generated `Bundle.module` can assert (and crash the host app) when
/// the resource path does not match its compile-time assumptions. Callers
/// should prefer this type — or `PermissionFlowLocalizer` — instead of
/// reading `Bundle.module` from UI or runtime code.
public enum PermissionFlowResources {
    /// SPM resource bundle name for the `PermissionFlow` target.
    public static let resourceBundleName = "PermissionFlow_PermissionFlow"

    /// Best-effort package resource bundle.
    ///
    /// Returns `nil` when the packaged resource cannot be located so callers
    /// can degrade gracefully (for example, fall back to English defaults)
    /// instead of trapping on `Bundle.module`.
    public static var packageBundle: Bundle? { resolvedPackageBundle }

    /// Resource bundle for consumers that need a non-optional `Bundle`.
    ///
    /// Prefer `packageBundle` when you can handle a missing resource bundle.
    /// This property falls back to `Bundle.main` only after package lookup
    /// fails so public API remains non-crashing in installed apps.
    public static var bundle: Bundle { packageBundle ?? .main }

    /// Localization key for the Accessibility privacy page title on
    /// macOS versions before 27.
    public static let accessibilityNameKey = "permission_flow.pane.accessibility"

    /// Localization key for the Accessibility privacy page title on
    /// macOS 27 and later, where System Settings labels the page
    /// "Device Control and Data Access".
    public static let deviceControlAndDataAccessNameKey = "permission_flow.pane.device_control_and_data_access"

    /// Localization key for the System Settings Accessibility page name.
    ///
    /// Returns `deviceControlAndDataAccessNameKey` on macOS 27 and later,
    /// and `accessibilityNameKey` on earlier versions. The settings URL
    /// and `Privacy_Accessibility` anchor are unchanged.
    public static func accessibilityName(
        operatingSystemVersion: OperatingSystemVersion = ProcessInfo.processInfo.operatingSystemVersion
    ) -> String {
        if operatingSystemVersion.majorVersion >= 27 {
            return deviceControlAndDataAccessNameKey
        }
        return accessibilityNameKey
    }

    /// SwiftUI-ready resource that follows the host app locale automatically.
    ///
    /// Prefer this in host UI:
    ///
    /// ```swift
    /// Text(PermissionFlowResources.accessibilityNameResource)
    /// ```
    public static var accessibilityNameResource: LocalizedStringResource {
        accessibilityNameResource(
            operatingSystemVersion: ProcessInfo.processInfo.operatingSystemVersion
        )
    }

    /// Same as `accessibilityNameResource`, with an explicit OS version for tests.
    public static func accessibilityNameResource(
        operatingSystemVersion: OperatingSystemVersion
    ) -> LocalizedStringResource {
        localizedStringResource(for: accessibilityName(operatingSystemVersion: operatingSystemVersion))
    }

    /// SwiftUI-ready resource for any packaged localization key.
    ///
    /// Pass a name key such as `accessibilityNameKey` or the result of
    /// `accessibilityName()`. The resource is backed by the package bundle,
    /// so SwiftUI follows the host app locale automatically.
    ///
    /// ```swift
    /// Text(PermissionFlowResources.localizedStringResource(for: PermissionFlowResources.accessibilityNameKey))
    /// ```
    public static func localizedStringResource(for nameKey: String) -> LocalizedStringResource {
        LocalizedStringResource(
            String.LocalizationValue(nameKey),
            bundle: .atURL(bundle.bundleURL)
        )
    }

    /// Resolves a packaged localization string for `nameKey`.
    ///
    /// Looks up the key in the package resource bundle's `.lproj` tables so
    /// host apps get PermissionFlow translations without calling
    /// `Bundle.module`. When `localeIdentifier` is `nil`, uses the preferred
    /// languages / current locale. Falls back to `defaultValue`, then the
    /// English package string, then `nameKey` itself.
    ///
    /// ```swift
    /// let name = PermissionFlowResources.localizedString(
    ///     for: PermissionFlowResources.accessibilityNameKey,
    ///     localeIdentifier: "zh-Hans"
    /// )
    /// ```
    public static func localizedString(
        for nameKey: String,
        defaultValue: String? = nil,
        localeIdentifier: String? = nil
    ) -> String {
        guard let packageBundle else {
            return defaultValue ?? nameKey
        }

        let fallback = defaultValue
            ?? localizedBundle(for: "en", in: packageBundle)?
                .localizedString(forKey: nameKey, value: nameKey, table: nil)
            ?? nameKey

        let resolvedLocaleIdentifier = localeIdentifier
            ?? Locale.preferredLanguages.first
            ?? Locale.current.identifier

        if let localized = localizedBundle(for: resolvedLocaleIdentifier, in: packageBundle) {
            return localized.localizedString(forKey: nameKey, value: fallback, table: nil)
        }

        return packageBundle.localizedString(forKey: nameKey, value: fallback, table: nil)
    }

    private static let resolvedPackageBundle: Bundle? = findPackageResourceBundle()

    /// Best matching `.lproj` bundle for `localeIdentifier` inside the
    /// packaged resource. Returns `nil` when no localization folder matches.
    private static func localizedBundle(for localeIdentifier: String?, in packageBundle: Bundle) -> Bundle? {
        guard let localeIdentifier, localeIdentifier.isEmpty == false else {
            return nil
        }

        let preferences = localizationPreferences(for: localeIdentifier)
        guard let localization = Bundle.preferredLocalizations(
            from: packageBundle.localizations,
            forPreferences: preferences
        ).first,
        let path = packageBundle.path(forResource: localization, ofType: "lproj") else {
            return nil
        }

        return Bundle(path: path)
    }

    private static func localizationPreferences(for localeIdentifier: String) -> [String] {
        let normalizedIdentifier = localeIdentifier.replacingOccurrences(of: "_", with: "-")
        let locale = Locale(identifier: normalizedIdentifier)

        var preferences = [normalizedIdentifier]
        if let identifier = locale.language.languageCode?.identifier {
            if let script = locale.language.script?.identifier {
                preferences.append("\(identifier)-\(script)")
            }
            preferences.append(identifier)
        }

        return Array(NSOrderedSet(array: preferences)) as? [String] ?? preferences
    }

    private static func findPackageResourceBundle() -> Bundle? {
        let bundleFileName = "\(resourceBundleName).bundle"

        var candidateDirectories: [URL] = []
        let marker = Bundle(for: PermissionFlowResourceBundleToken.self)

        candidateDirectories.append(contentsOf: [
            Bundle.main.resourceURL,
            Bundle.main.bundleURL,
            marker.resourceURL,
            marker.bundleURL,
            marker.bundleURL.appendingPathComponent("Contents/Resources"),
            marker.bundleURL.appendingPathComponent("Resources"),
            Bundle.main.bundleURL.appendingPathComponent("Contents/Resources"),
            Bundle.main.bundleURL.deletingLastPathComponent()
        ].compactMap { $0 })

        // De-duplicate while preserving order.
        var seen = Set<String>()
        candidateDirectories = candidateDirectories.filter { url in
            seen.insert(url.standardizedFileURL.path).inserted
        }

        for directory in candidateDirectories {
            let directURL = directory.appendingPathComponent(bundleFileName)
            if let bundle = Bundle(url: directURL) {
                return bundle
            }

            let nestedURL = directory
                .appendingPathComponent("Contents")
                .appendingPathComponent("Resources")
                .appendingPathComponent(bundleFileName)
            if let bundle = Bundle(url: nestedURL) {
                return bundle
            }
        }

        // Scan already-loaded bundles/frameworks for the packaged resource.
        for host in Bundle.allBundles + Bundle.allFrameworks {
            if host.bundleURL.lastPathComponent == bundleFileName {
                return host
            }
            if let url = host.url(forResource: resourceBundleName, withExtension: "bundle"),
               let bundle = Bundle(url: url) {
                return bundle
            }
        }

        return nil
    }
}

/// Marker type used only for `Bundle(for:)` resolution. Must stay in this
/// module so the returned bundle points at PermissionFlow's binary/product.
private final class PermissionFlowResourceBundleToken {}
