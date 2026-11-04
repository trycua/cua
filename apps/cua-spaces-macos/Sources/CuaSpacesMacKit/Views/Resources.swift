// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation

/// This module's resources (`Sources/CuaSpacesMacKit/Resources`), found
/// without SwiftPM's generated `Bundle.module`.
///
/// `Bundle.module` looks for `CuaSpacesMac_CuaSpacesMacKit.bundle` at the
/// main bundle's root, then at the absolute path of the build directory, and
/// calls `fatalError` when neither exists. A signed app cannot keep the
/// bundle at its root (codesign refuses unsealed content there), so
/// `scripts/build-app.sh` puts it in `Contents/Resources`, where the
/// accessor never looks: the app worked only on the machine that built it.
/// This lookup searches `Contents/Resources` first and returns nil instead
/// of crashing. Never use `Bundle.module` in this target
/// (`ResourceLookupTests` checks the sources).
public enum ModuleResources {
    /// The resource bundle's file name, as SwiftPM names it
    /// (`<package>_<target>.bundle`).
    public static let bundleName = "CuaSpacesMac_CuaSpacesMacKit.bundle"

    /// The resource bundle, or nil when none of the places it is installed
    /// or built to has it.
    public static let bundle: Bundle? = candidates().lazy.compactMap { Bundle(url: $0) }.first

    /// Where the bundle can be, in order: the app's `Contents/Resources`
    /// (`build-app.sh`, Xcode), the main bundle's root and executable
    /// directory (`swift run`), and next to the bundle holding this code
    /// (`swift test`: the `.xctest` bundle sits in the build directory).
    static func candidates(main: Bundle = .main, code: Bundle = Bundle(for: Marker.self)) -> [URL] {
        let dirs: [URL?] = [
            main.resourceURL,
            main.bundleURL,
            main.executableURL?.deletingLastPathComponent(),
            code.resourceURL,
            code.bundleURL,
            code.bundleURL.deletingLastPathComponent(),
        ]
        var out: [URL] = []
        for dir in dirs.compactMap({ $0?.standardizedFileURL }) {
            let url = dir.appendingPathComponent(bundleName, isDirectory: true)
            if !out.contains(url) { out.append(url) }
        }
        return out
    }

    /// A resource's URL, or nil.
    public static func url(forResource name: String, withExtension ext: String?) -> URL? {
        bundle?.url(forResource: name, withExtension: ext)
    }

    final class Marker {}
}
