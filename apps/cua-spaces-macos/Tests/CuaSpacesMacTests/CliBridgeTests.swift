// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesMacKit
import CuaSpacesFFI
import Foundation
import Testing

/// The bundled CLI on PATH (`cli.plan`, `cli.install`, `settings.get`'s
/// `cli`): the app core's installer, on temp folders and a fake `cua`.
@MainActor
struct CliBridgeTests {
    private func bridge(_ dir: URL, bundled: String?) -> WebUIBridge {
        let m = AppModel(backend: FixtureSpacesBackend(), keyvault: KeyvaultModel(client: nil),
                         onboarding: OnboardingModel(statePath: dir.appendingPathComponent("onboarding.json").path),
                         settingsPath: dir.appendingPathComponent("settings.json").path,
                         telemetry: FixtureTelemetry(), devices: FixtureDevices())
        let b = WebUIBridge(model: m, allowedOrigins: [])
        let bin = dir.appendingPathComponent("bin").path
        let profile = dir.appendingPathComponent(".zshrc").path
        b.makeCliInstaller = { AppCliInstaller.withPaths(bundled: bundled, binDir: bin, pathEnv: "/usr/bin", profile: profile) }
        return b
    }

    /// A fake bundled `cua` that answers `--version` as the real one does.
    private func fakeCua(_ dir: URL) throws -> String {
        let app = dir.appendingPathComponent("App/cua")
        try FileManager.default.createDirectory(at: app.deletingLastPathComponent(), withIntermediateDirectories: true)
        try "#!/bin/sh\necho 'cua 0.9.0'\n".write(to: app, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: app.path)
        return app.path
    }

    private func tempDir() throws -> URL {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("cua-cli-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }

    @Test func aDiskImageInstallPlansThenInstallsOnlyWhenAsked() async throws {
        let dir = try tempDir()
        defer { try? FileManager.default.removeItem(at: dir) }
        let b = bridge(dir, bundled: try fakeCua(dir))
        let plan = try #require(try await b.handle("cli.plan", [:]) as? [String: Any])
        #expect(plan["target"] as? String == dir.appendingPathComponent("bin/cua").path)
        #expect(plan["onPath"] as? Bool == false)
        #expect(plan["found"] is NSNull)
        // Nothing written by the plan.
        #expect(!FileManager.default.fileExists(atPath: dir.appendingPathComponent("bin/cua").path))
        let settings = b.settings()["cli"] as? [String: Any]
        #expect(settings?["name"] as? String == "cua")
        #expect(settings?["installed"] as? Bool == false)

        let done = try #require(try await b.handle("cli.install", ["modifyPath": true]) as? [String: Any])
        #expect(done["upToDate"] as? Bool == true)
        #expect(FileManager.default.fileExists(atPath: dir.appendingPathComponent("bin/cua").path))
        #expect(try String(contentsOf: dir.appendingPathComponent(".zshrc"), encoding: .utf8).contains(dir.appendingPathComponent("bin").path))
        #expect((b.settings()["cli"] as? [String: Any])?["installed"] as? Bool == true)
    }

    @Test func aBuildWithoutABundledCliHasNothingToInstall() async throws {
        let dir = try tempDir()
        defer { try? FileManager.default.removeItem(at: dir) }
        let b = bridge(dir, bundled: nil)
        let plan = try #require(try await b.handle("cli.plan", [:]) as? [String: Any])
        #expect(plan["source"] is NSNull)
        await #expect(throws: (any Error).self) { _ = try await b.handle("cli.install", ["modifyPath": false]) }
        await #expect(throws: (any Error).self) { _ = try await b.handle("cli.install", [:]) }
    }
}
