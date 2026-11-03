// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesMacKit
import Foundation
import Testing

/// `CUA_ENV`: production changes nothing; staging gets its own home; a
/// bundle's `CuaEnv` default (a development build can set it) applies only when
/// `CUA_ENV` is unset.
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

    @Test func bundleDefaultStagingAppliesWhenCuaEnvIsUnset() {
        #expect(TargetEnvironment.pinned(env: [:], bundleEnv: "staging", home: "/Users/ada")
            == ["CUA_ENV": "staging", "CUA_HOME": "/Users/ada/.cua/envs/staging"])
    }
}

/// `CUA_CHANNEL=local` (Cua Spaces Local): its own home, staging by default
/// (and `envs/prod` below it under the production opt-in), the same three
/// variables for every child; Stable is untouched.
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

    @Test func localHomeHasNoEnvironmentBelowItOnStaging() {
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

    @Test func productionIsAnExplicitOptInWithAHomeOfItsOwnBelowLocal() {
        let out = pinned(env: ["CUA_ENV": "prod"])
        #expect(out["CUA_ENV"] == nil)
        #expect(out["CUA_HOME"] == "/Users/ada/.cua-local/envs/prod")
        #expect(out["CUA_CHANNEL"] == "local")
        // Spelled any way cua-home accepts.
        #expect(pinned(env: ["CUA_ENV": " Production "])["CUA_HOME"] == "/Users/ada/.cua-local/envs/prod")
        // From the bundle, when `CUA_ENV` is unset.
        let bundle = pinned(bundleEnv: "prod")
        #expect(bundle["CUA_ENV"] == "prod")
        #expect(bundle["CUA_HOME"] == "/Users/ada/.cua-local/envs/prod")
        // An absolute bundle home gets the same folder below it.
        #expect(pinned(env: ["CUA_ENV": "prod"], bundleHome: "/Volumes/x/.cua-local")["CUA_HOME"]
            == "/Volumes/x/.cua-local/envs/prod")
    }

    @Test func aMistypedEnvironmentIsStagingNotProduction() {
        // As in cua-home: Local never falls back to production.
        #expect(pinned(env: ["CUA_ENV": "prd"])["CUA_HOME"] == "/Users/ada/.cua-local")
    }

    @Test func explicitStagingKeepsTheLocalHome() {
        #expect(pinned(env: ["CUA_ENV": "staging"])["CUA_HOME"] == "/Users/ada/.cua-local")
    }

    /// The four (channel, environment) pairs, each with a home of its own
    /// (`CuaChannel::env_home_dir`), and no Local home inside Stable's.
    @Test func everyChannelAndEnvironmentHasItsOwnHome() {
        func home(_ env: [String: String], channel: String?) -> String {
            let out = TargetEnvironment.pinned(
                env: env, bundleEnv: nil, home: "/Users/ada",
                bundleChannel: channel, bundleHome: channel == "local" ? "~/.cua-local" : nil)
            // Stable production pins nothing: its home is `~/.cua`.
            return out["CUA_HOME"] ?? "/Users/ada/.cua"
        }
        let homes = [
            home([:], channel: nil),
            home(["CUA_ENV": "staging"], channel: nil),
            home(["CUA_ENV": "staging"], channel: "local"),
            home(["CUA_ENV": "prod"], channel: "local"),
        ]
        #expect(homes == [
            "/Users/ada/.cua",
            "/Users/ada/.cua/envs/staging",
            "/Users/ada/.cua-local",
            "/Users/ada/.cua-local/envs/prod",
        ])
        #expect(Set(homes).count == 4)
        for stable in homes[0..<2] {
            for local in homes[2..<4] {
                #expect(!stable.hasPrefix(local + "/") && !local.hasPrefix(stable + "/"))
            }
        }
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
        // A bundle with a staging default (a development build): the existing behaviour.
        #expect(pinned(bundleChannel: nil, bundleHome: nil)
            == ["CUA_ENV": "staging", "CUA_HOME": "/Users/ada/.cua/envs/staging"])
    }
}
