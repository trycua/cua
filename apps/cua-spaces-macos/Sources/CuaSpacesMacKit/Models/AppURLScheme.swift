// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation

/// The URL scheme this app answers to: `cua` for the released app, `cua-local`
/// for Cua Spaces Local. `scripts/build-app.sh --channel local` stamps it as
/// Info.plist `CuaUrlScheme` (`CuaChannel::url_scheme` in cua-home) and
/// registers it in `CFBundleURLTypes`, in place of `cua`; a bundle without the
/// key is the released app. The two apps run side by side, so each must
/// handle only its own links: `cua://add/<code>` belongs to Stable and
/// `cua-local://add/<code>` to Local (CUA-1776).
public enum AppURLScheme {
    /// The released app's scheme, and the default when the bundle has no
    /// `CuaUrlScheme`.
    public static let stable = "cua"

    /// The scheme `info` (an Info.plist) names: `CuaUrlScheme` (trimmed,
    /// lower case; a URL's scheme is case-insensitive), else `cua`.
    public static func resolve(info: [String: Any]?) -> String {
        let value = ((info?["CuaUrlScheme"] as? String) ?? "")
            .trimmingCharacters(in: .whitespaces).lowercased()
        return value.isEmpty ? stable : value
    }

    /// This app's scheme.
    public static var current: String { resolve(info: Bundle.main.infoDictionary) }
}
