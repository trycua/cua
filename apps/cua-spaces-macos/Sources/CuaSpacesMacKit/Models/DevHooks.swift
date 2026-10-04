// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation

/// The recording and test hooks (`CUA_SPACES_FIXTURES`, `CUA_SPACES_START_VIEW`,
/// `CUA_SPACES_BROWSER`, `CUA_SPACES_ACTIVATE`, ...). Captures, UI tests and
/// end-to-end runs use debug builds; a release build reads none of them, so
/// its environment can't steer what it shows, creates or opens.
///
/// Every `CUA_SPACES_*` read in this app goes through here
/// (`DevHooksTests` checks the sources).
public enum DevHooks {
    /// Whether this build reads the hooks at all.
    public static var enabled: Bool {
        #if DEBUG
        return true
        #else
        return false
        #endif
    }

    /// The hooks present in `environment`: the `CUA_SPACES_*` entries in a
    /// debug build, nothing in a release build.
    public static func filter(_ environment: [String: String], enabled: Bool = DevHooks.enabled) -> [String: String] {
        guard enabled else { return [:] }
        return environment.filter { $0.key.hasPrefix("CUA_SPACES_") }
    }

    /// The hooks from the process environment.
    public static var environment: [String: String] {
        filter(ProcessInfo.processInfo.environment)
    }

    /// One hook's value, or nil (always nil in a release build).
    public static func value(_ name: String) -> String? {
        environment[name]
    }
}
