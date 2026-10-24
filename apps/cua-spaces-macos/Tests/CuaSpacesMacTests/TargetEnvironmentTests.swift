// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesMacKit
import Foundation
import Testing

/// `CUA_ENV`: production changes nothing; staging gets its own home; a beta
/// build's bundle default applies only when `CUA_ENV` is unset.
@Suite("Target environment")
struct TargetEnvironmentTests {
    @Test func productionChangesNothing() {
        #expect(TargetEnvironment.pinned(env: [:], bundleEnv: nil, home: "/Users/ada").isEmpty)
        #expect(TargetEnvironment.pinned(env: ["CUA_ENV": "prod"], bundleEnv: "staging", home: "/Users/ada").isEmpty)
    }

    @Test func stagingUsesItsOwnHome() {
        #expect(TargetEnvironment.pinned(env: ["CUA_ENV": "Staging"], bundleEnv: nil, home: "/Users/ada")
            == ["CUA_HOME": "/Users/ada/.cua/envs/staging"])
        #expect(TargetEnvironment.pinned(env: ["CUA_ENV": "staging", "CUA_HOME": "/tmp/h"], bundleEnv: nil, home: "/Users/ada")
            .isEmpty)
    }

    @Test func betaBundleDefaultsToStaging() {
        #expect(TargetEnvironment.pinned(env: [:], bundleEnv: "staging", home: "/Users/ada")
            == ["CUA_ENV": "staging", "CUA_HOME": "/Users/ada/.cua/envs/staging"])
    }
}

/// `CUA_CHANNEL=local` (Cua Spaces Local): its own home, staging by default,
/// the same three variables for every child; Stable is untouched.
@Suite("Target environment, Local channel")
struct LocalChannelTargetEnvironmentTests {

    private func pinned(
        env: [String: String] = [:], bundleChannel: String? = "local",
        bundleEnv: String? = "staging", bundleHome: String? = "~/.cua-local"
    ) -> [String: String] {
        TargetEnvironment.pinned(
            env: env, bundleEnv: bundleEnv, home: "/Users/ada",
            bundleChannel: bundleChannel, bundleHome: bundleHome)
    }

    @Test func localBundleSetsChannelStagingAndItsOwnHome() {
        #expect(pinned() == [
            "CUA_CHANNEL": "local", "CUA_ENV": "staging", "CUA_HOME": "/Users/ada/.cua-local",
        ])
    }

    @Test func localHomeHasNoEnvironmentBelowIt() {
        let home = pinned()["CUA_HOME"] ?? ""
        #expect(home == "/Users/ada/.cua-local")
        #expect(!home.contains("envs"))
    }

    @Test func localDefaultsToStagingEvenWithoutTheBundleKeys() {
        #expect(pinned(bundleEnv: nil, bundleHome: nil) == [
            "CUA_CHANNEL": "local", "CUA_ENV": "staging", "CUA_HOME": "/Users/ada/.cua-local",
        ])
        #expect(pinned(bundleEnv: "", bundleHome: "")["CUA_ENV"] == "staging")
    }

    @Test func productionIsAnExplicitOptInAndKeepsTheLocalHome() {
        let out = pinned(env: ["CUA_ENV": "prod"])
        #expect(out["CUA_ENV"] == nil)
        #expect(out["CUA_HOME"] == "/Users/ada/.cua-local")
        #expect(out["CUA_CHANNEL"] == "local")
    }

    @Test func setVariablesWin() {
        #expect(pinned(env: ["CUA_CHANNEL": "local", "CUA_ENV": "staging", "CUA_HOME": "/tmp/h"]).isEmpty)
        #expect(pinned(env: ["CUA_HOME": "/tmp/h"])["CUA_HOME"] == nil)
    }

    @Test func aBlankVariableCountsAsUnset() {
        let out = pinned(env: ["CUA_CHANNEL": " ", "CUA_ENV": "", "CUA_HOME": " "])
        #expect(out == ["CUA_CHANNEL": "local", "CUA_ENV": "staging", "CUA_HOME": "/Users/ada/.cua-local"])
    }

    @Test func anAbsoluteBundleHomeIsKept() {
        #expect(pinned(bundleHome: "/Volumes/x/.cua-local")["CUA_HOME"] == "/Volumes/x/.cua-local")
    }

    @Test func theChannelVariableOverridesTheBundle() {
        // `CUA_CHANNEL=stable` on a Local bundle is Stable's rule, as in cua-home.
        let out = pinned(env: ["CUA_CHANNEL": "stable"], bundleEnv: nil)
        #expect(out.isEmpty)
        // An unknown value is not a channel: the bundle decides.
        #expect(pinned(env: ["CUA_CHANNEL": "dev"])["CUA_CHANNEL"] == "local")
    }

    @Test func stableIsUnchanged() {
        // No bundle channel (every Stable build), or an explicit `stable`.
        #expect(pinned(bundleChannel: nil, bundleEnv: nil, bundleHome: nil).isEmpty)
        #expect(pinned(bundleChannel: "stable", bundleEnv: nil, bundleHome: nil).isEmpty)
        #expect(pinned(env: ["CUA_ENV": "staging"], bundleChannel: nil, bundleEnv: nil, bundleHome: nil)
            == ["CUA_HOME": "/Users/ada/.cua/envs/staging"])
        // A beta bundle: the existing behaviour.
        #expect(pinned(bundleChannel: nil, bundleHome: nil)
            == ["CUA_ENV": "staging", "CUA_HOME": "/Users/ada/.cua/envs/staging"])
    }
}
