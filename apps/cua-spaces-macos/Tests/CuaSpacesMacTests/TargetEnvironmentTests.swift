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

    @Test func devBundleGetsItsOwnHomeAndServiceLabel() {
        let pinned = TargetEnvironment.pinned(
            env: [:], bundleEnv: "staging", home: "/Users/ada",
            bundleHome: "~/.cua-dev/feat-x", bundleServiceSuffix: "dev-feat-x")
        #expect(pinned == ["CUA_ENV": "staging", "CUA_HOME": "/Users/ada/.cua-dev/feat-x", "CUA_SERVICE_SUFFIX": "dev-feat-x"])
    }

    @Test func devBundleDefaultsYieldToTheEnvironment() {
        let pinned = TargetEnvironment.pinned(
            env: ["CUA_HOME": "/tmp/h", "CUA_SERVICE_SUFFIX": "x", "CUA_ENV": "staging"], bundleEnv: "staging",
            home: "/Users/ada", bundleHome: "~/.cua-dev/feat-x", bundleServiceSuffix: "dev-feat-x")
        #expect(pinned.isEmpty)
    }
}
