// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation

/// Which Cua channel and deployment the app runs as: `CUA_CHANNEL` (`stable`,
/// the default, or `local`, "Cua Spaces Local") and `CUA_ENV` (`prod`, the
/// default, or `staging`). The SDK resolves every endpoint and name from
/// them (`cua-home`'s `CuaChannel` and `CuaEnv`). The app only settles them
/// before anything starts, so the daemon, the helper and every other child
/// inherit the same values:
///
/// - `CUA_CHANNEL` unset: the bundle's `CuaChannel` Info.plist value, which
///   `scripts/build-app.sh --channel local` sets to `local`; Stable builds
///   have none. A Local app then also settles:
///   - `CUA_ENV` unset: the bundle's `CuaEnv` (`staging`), else `staging`:
///     Local never talks to production unless `CUA_ENV=prod` says so;
///   - `CUA_HOME` unset: the bundle's `CuaHome` (`~/.cua-local`), else
///     `~/.cua-local`, whatever `CUA_ENV` is, so it never shares Stable's
///     home, daemon or sockets;
/// - on Stable, `CUA_ENV` unset: the bundle's `CuaEnv` Info.plist value, which
///   a development build can set to `staging`; release builds, betas
///   included, have none (`scripts/build-release.sh` removes it), so they use
///   production;
/// - on Stable, `CUA_HOME` unset under staging: `~/.cua/envs/staging`, so the
///   daemon it supervises and every file it reads use the same home as the
///   SDK.
///
/// A variable that is already set always wins (a blank one counts as unset
/// for a Local app), as in `cua-home`.
public enum TargetEnvironment {
    /// The channel a value names: `stable` or `local` (any case), else nil.
    private static func channel(_ value: String?) -> String? {
        let name = (value ?? "").trimmingCharacters(in: .whitespaces).lowercased()
        return name == "stable" || name == "local" ? name : nil
    }

    private static func trimmed(_ value: String?) -> String {
        (value ?? "").trimmingCharacters(in: .whitespaces)
    }

    private static func isBlank(_ value: String?) -> Bool {
        trimmed(value).isEmpty
    }

    /// `path` with a leading `~/` replaced by `home`.
    private static func expand(_ path: String, home: String) -> String {
        path.hasPrefix("~/")
            ? URL(fileURLWithPath: home, isDirectory: true)
                .appendingPathComponent(String(path.dropFirst(2)), isDirectory: true).path
            : path
    }

    /// The variables to set for `env` and the bundle's `CuaChannel`, `CuaEnv`
    /// and `CuaHome` values.
    public static func pinned(
        env: [String: String], bundleEnv: String?, home: String,
        bundleChannel: String? = nil, bundleHome: String? = nil
    ) -> [String: String] {
        var out: [String: String] = [:]
        var name = channel(env["CUA_CHANNEL"])
        if name == nil, let bundle = channel(bundleChannel) {
            name = bundle
            // Only a Local bundle sets it: Stable builds never did.
            if bundle == "local" { out["CUA_CHANNEL"] = bundle }
        }
        if name == "local" {
            if isBlank(env["CUA_ENV"]) {
                out["CUA_ENV"] = isBlank(bundleEnv) ? "staging" : trimmed(bundleEnv)
            }
            if isBlank(env["CUA_HOME"]) {
                out["CUA_HOME"] = expand(isBlank(bundleHome) ? "~/.cua-local" : trimmed(bundleHome), home: home)
            }
            return out
        }
        var envName = env["CUA_ENV"]
        if envName == nil, let bundleEnv, !bundleEnv.isEmpty {
            envName = bundleEnv
            out["CUA_ENV"] = bundleEnv
        }
        let normalized = (envName ?? "").trimmingCharacters(in: .whitespaces).lowercased()
        if normalized == "staging", (env["CUA_HOME"] ?? "").isEmpty {
            out["CUA_HOME"] = URL(fileURLWithPath: home, isDirectory: true)
                .appendingPathComponent(".cua/envs/staging", isDirectory: true).path
        }
        return out
    }

    /// Applies [`pinned`] to this process. Call first in `main`.
    public static func pin(bundle: Bundle = .main) {
        apply(pinned(bundle: bundle))
    }

    /// Applies [`pinned`] to this process only when the bundle is a Local
    /// build: `CUA_CHANNEL`, `CUA_ENV` and `CUA_HOME`. For the entry points
    /// that run before [`pin`] (the login item, which starts `cua-spacesd`
    /// with this process's environment). A Stable bundle changes nothing.
    public static func pinChannel(bundle: Bundle = .main) {
        guard channel(bundle.object(forInfoDictionaryKey: "CuaChannel") as? String) == "local" else { return }
        apply(pinned(bundle: bundle))
    }

    private static func pinned(bundle: Bundle) -> [String: String] {
        let env = ProcessInfo.processInfo.environment
        return pinned(
            env: env,
            bundleEnv: bundle.object(forInfoDictionaryKey: "CuaEnv") as? String,
            home: env["HOME"] ?? NSHomeDirectory(),
            bundleChannel: bundle.object(forInfoDictionaryKey: "CuaChannel") as? String,
            bundleHome: bundle.object(forInfoDictionaryKey: "CuaHome") as? String)
    }

    private static func apply(_ vars: [String: String]) {
        for (key, value) in vars { setenv(key, value, 1) }
    }
}
