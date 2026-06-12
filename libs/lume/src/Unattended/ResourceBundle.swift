import Foundation

extension Bundle {
    /// Custom resource bundle accessor that works both for standalone binaries
    /// (where SPM places the bundle next to the executable) and for .app bundles
    /// (where the bundle lives in Contents/Resources/).
    ///
    /// SPM's auto-generated `Bundle.module` only checks `Bundle.main.bundleURL`
    /// (the .app root), which doesn't match `Contents/Resources/` in a .app bundle.
    /// This accessor checks `resourceURL` first, then `bundleURL`, then the build path.
    static let lumeResources: Bundle = {
        let bundleName = "lume_lume.bundle"

        // 1. .app bundle: Contents/Resources/
        if let resourceURL = Bundle.main.resourceURL {
            let path = resourceURL.appendingPathComponent(bundleName).path
            if let bundle = Bundle(path: path) {
                return bundle
            }
        }

        // 2. Standalone binary: next to the executable
        let mainPath = Bundle.main.bundleURL.appendingPathComponent(bundleName).path
        if let bundle = Bundle(path: mainPath) {
            return bundle
        }

        // 3. Development fallback: SPM build directory
        #if DEBUG
        // During development, try the build directory
        let buildPath = Bundle.main.bundleURL.appendingPathComponent(bundleName).path
        if let bundle = Bundle(path: buildPath) {
            return bundle
        }
        #endif

        fatalError("Could not load resource bundle '\(bundleName)' from resourceURL or bundleURL")
    }()
}
