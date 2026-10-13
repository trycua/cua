// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation

/// Which Cua deployment the app talks to: `CUA_ENV` (`prod`, the default, or
/// `staging`). The SDK resolves every endpoint from it. The app only settles
/// it before anything starts:
///
/// - `CUA_ENV` unset: the bundle's `CuaEnv` Info.plist value, which a beta
///   channel build sets to `staging` (`scripts/build-release.sh` with
///   `CUA_CHANNEL=beta`); release builds have none (production);
/// - `CUA_HOME` unset under staging: `~/.cua/envs/staging`, so the daemon it
///   supervises and every file it reads use the same home as the SDK.
public enum TargetEnvironment {
    /// The variables to set for `env` and the bundle's `CuaEnv` value.
    public static func pinned(env: [String: String], bundleEnv: String?, home: String) -> [String: String] {
        var out: [String: String] = [:]
        var name = env["CUA_ENV"]
        if name == nil, let bundleEnv, !bundleEnv.isEmpty {
            name = bundleEnv
            out["CUA_ENV"] = bundleEnv
        }
        let normalized = (name ?? "").trimmingCharacters(in: .whitespaces).lowercased()
        if normalized == "staging", (env["CUA_HOME"] ?? "").isEmpty {
            out["CUA_HOME"] = URL(fileURLWithPath: home, isDirectory: true)
                .appendingPathComponent(".cua/envs/staging", isDirectory: true).path
        }
        return out
    }

    /// Applies [`pinned`] to this process. Call first in `main`.
    public static func pin(bundle: Bundle = .main) {
        let env = ProcessInfo.processInfo.environment
        let bundleEnv = bundle.object(forInfoDictionaryKey: "CuaEnv") as? String
        for (key, value) in pinned(env: env, bundleEnv: bundleEnv, home: env["HOME"] ?? NSHomeDirectory()) {
            setenv(key, value, 1)
        }
    }
}
