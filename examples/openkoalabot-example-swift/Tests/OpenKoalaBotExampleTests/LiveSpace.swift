// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation
import Testing
@testable import OpenKoalaBotExample

/// How the live suites reach a Space. Nothing here creates one.
///
/// * `OPENKOALABOTS_TEST_SPACE_URL` (+ `OPENKOALABOTS_TEST_SPACE_TOKEN`): a machine
///   that runs cua-spacesd (for example a local linux
///   container). It is added to a **temp** Spaces registry in an embedded
///   runtime (never `~/.cua`), pinned for the run, and unregistered at exit.
/// * `OPENKOALABOTS_TEST_SPACE`: an existing Space id, reached through
///   `SDKSpacesClient.backendFromEnvironment()` (a running `cua daemon`, or
///   embedded with `OPENKOALABOTS_SPACES=embedded`).
///
/// With neither set, every live test skips.
enum LiveSpace {
    struct Target: @unchecked Sendable {
        let client: SDKSpacesClient
        let space: String
        /// `macos`, `linux`, … — the live suites grew up on a macOS Space and
        /// the few assertions that are about macOS (Terminal, LaunchAgents)
        /// run only there.
        let os: String
    }

    static let target: Target? = {
        let env = ProcessInfo.processInfo.environment
        let make: @Sendable () async throws -> Target? = {
            if let url = env["OPENKOALABOTS_TEST_SPACE_URL"], !url.isEmpty {
                let home = FileManager.default.temporaryDirectory
                    .appendingPathComponent("openkoalabots-tests-\(UUID().uuidString)").path
                let client = try SDKSpacesClient(backend: .embedded(spacesHome: home))
                let id = try await client.addSpace(url: url, token: env["OPENKOALABOTS_TEST_SPACE_TOKEN"],
                                                   name: "openkoalabots-tests")
                SDKSpacesClient.pinnedSpace = id
                return Target(client: client, space: id, os: try await osOf(client, id))
            }
            guard let pinned = SDKSpacesClient.overriddenSpace else { return nil }
            let client = try SDKSpacesClient(backend: SDKSpacesClient.backendFromEnvironment())
            return Target(client: client, space: pinned, os: try await osOf(client, pinned))
        }
        return SpaceHygiene.sync { try await make() } ?? nil
    }()

    /// The Space's OS: the inventory's answer, or the guest's own `uname`
    /// for a direct Space, whose registration does not carry one.
    private static func osOf(_ client: SDKSpacesClient, _ id: String) async throws -> String {
        if let os = try await client.listSpaces().first(where: { $0.id == id })?.os, !os.isEmpty {
            return os
        }
        let out = try await client.raw("space_bash", ["space": id, "command": "uname -s"])
        let text = (out as? [String: Any])?["stdout"] as? String ?? (out as? String ?? "")
        return text.lowercased().contains("darwin") ? "macos"
            : text.lowercased().contains("linux") ? "linux" : ""
    }

    /// The live Space, or a skip that says how to provide one.
    static func require(_ what: String = "the live suite") throws -> Target {
        SpaceHygiene.installTestDefaults()
        guard let target else {
            try XCTSkipNow("""
                set OPENKOALABOTS_TEST_SPACE_URL (+ OPENKOALABOTS_TEST_SPACE_TOKEN) to a spacesd, \
                or \(SDKSpacesClient.spaceOverrideVariable) to an existing Space, to run \(what). \
                Unset, these tests skip rather than create a sandbox.
                """)
        }
        return target
    }
}

/// Per-test cleanup for the live suites: XCTest's async `tearDown`.
///
/// swift-testing has no async teardown on a suite instance, so the closures a
/// test registers are collected in a task-local box and drained by the
/// `.liveSpace` trait after the test body returns — pass, fail or throw.
/// Closures capture the suite instance strongly so it outlives its body until
/// its cleanup has run.
final class LiveCleanup: @unchecked Sendable {
    @TaskLocal static var current = LiveCleanup()

    private let lock = NSLock()
    private var undo: [() async -> Void] = []

    func append(_ body: @escaping () async -> Void) {
        lock.lock(); undo.append(body); lock.unlock()
    }

    func drain() async {
        lock.lock(); let all = undo.reversed(); undo = []; lock.unlock()
        for body in all { await body() }
    }
}

/// `@Suite(.liveSpace)`: per test, drains `LiveCleanup`; per suite, the
/// sweeper (`SpaceSweeper`) snapshots the Space before the suite and fails the
/// suite if runs or windows it created are left behind.
struct LiveSpaceTrait: SuiteTrait, TestTrait, TestScoping {
    var isRecursive: Bool { true }

    /// Once around each suite (the sweeper) and once around each test case
    /// (cleanup). The default only scopes test cases.
    func scopeProvider(for test: Test, testCase: Test.Case?) -> Self? {
        test.isSuite ? (testCase == nil ? self : nil) : (testCase != nil ? self : nil)
    }

    func provideScope(for test: Test, testCase: Test.Case?,
                      performing function: @Sendable () async throws -> Void) async throws {
        if test.isSuite {
            let sweeper = SpaceSweeper()
            await sweeper.snapshot()
            do { try await function() } catch { await sweeper.sweep(); throw error }
            await sweeper.sweep()
            return
        }
        let box = LiveCleanup()
        try await LiveCleanup.$current.withValue(box) {
            do { try await function() } catch { await box.drain(); throw error }
            await box.drain()
        }
    }
}

extension Trait where Self == LiveSpaceTrait {
    static var liveSpace: LiveSpaceTrait { LiveSpaceTrait() }
}
