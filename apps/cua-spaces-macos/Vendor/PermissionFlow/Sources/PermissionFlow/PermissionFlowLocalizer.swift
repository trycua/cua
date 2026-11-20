#if os(macOS)
import Foundation

@available(macOS 13.0, *)
enum PermissionFlowLocalizer {
    /// Resolves a localized string from the best matching `.lproj` bundle for
    /// the injected locale. Never touches `Bundle.module`; uses the resilient
    /// packaged-resource lookup and falls back to `defaultValue` when the
    /// resource bundle cannot be found.
    static func string(
        _ key: String,
        defaultValue: String,
        localeIdentifier: String?
    ) -> String {
        PermissionFlowResources.localizedString(
            for: key,
            defaultValue: defaultValue,
            localeIdentifier: localeIdentifier
        )
    }
}
#endif
