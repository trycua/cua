// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
@testable import CuaSpacesMacKit
import CuaSpacesFFI
import Foundation
import Testing

/// Settings, Advanced's runtime rows on the Mac: one per kind of Space
/// (Containers, Virtual machines, MicroVMs), from the page's first
/// `settings.get` on; Already installed with what is installed, the path
/// in use and its check; each choice written to the per-kind setting.
///
/// The `settings.get` answers here are also the web UI's fixtures
/// (`apps/cua-spaces-web/src/__tests__/fixtures/mac-host-settings-*.json`):
/// its Advanced tab is tested on exactly what this host answers.
/// `UPDATE_WEB_FIXTURES=1` rewrites them; otherwise they must match.
@Suite("Runtime settings")
@MainActor
struct RuntimeSettingsTests {
    static let fixtures = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        .appendingPathComponent("../cua-spaces-web/src/__tests__/fixtures").standardizedFileURL

    init() { _ = NSApplication.shared }

    func bridge(_ backend: FixtureSpacesBackend = FixtureSpacesBackend()) -> (WebUIBridge, AppModel) {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("cua-runtimes-\(UUID().uuidString)")
        let model = AppModel(backend: backend, keyvault: KeyvaultModel(client: nil),
                             onboarding: OnboardingModel(statePath: dir.appendingPathComponent("onboarding.json").path),
                             settingsPath: dir.appendingPathComponent("settings.json").path,
                             telemetry: FixtureTelemetry())
        return (WebUIBridge(model: model, allowedOrigins: []), model)
    }

    func rows(_ answer: [String: Any]) -> [[String: Any]] {
        let page = answer["page"] as? [String: Any]
        let sections = page?["sections"] as? [[String: Any]] ?? []
        return sections.first { $0["id"] as? String == "runtimes" }?["rows"] as? [[String: Any]] ?? []
    }

    func ids(_ answer: [String: Any]) -> [String] { rows(answer).compactMap { $0["id"] as? String } }

    func row(_ answer: [String: Any], _ id: String) -> [String: Any]? {
        rows(answer).first { $0["id"] as? String == id }
    }

    func get(_ bridge: WebUIBridge) async throws -> [String: Any] {
        try #require(try await bridge.handle("settings.get", [:]) as? [String: Any])
    }

    /// The answer the web UI's test reads: the page's General and Runtimes
    /// sections and the runtimes as read (the rest depends on this Mac).
    func checkFixture(_ answer: [String: Any], _ name: String) throws {
        let page = try #require(answer["page"] as? [String: Any])
        let sections = (page["sections"] as? [[String: Any]] ?? []).filter {
            ["general", "runtimes"].contains($0["id"] as? String ?? "")
        }
        let subset: [String: Any] = [
            "page": ["title": page["title"] ?? "Settings", "sections": sections] as [String: Any],
            "runtimes": answer["runtimes"] ?? NSNull(),
        ]
        var data = try JSONSerialization.data(withJSONObject: subset, options: [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes])
        data.append(0x0A)
        let file = Self.fixtures.appendingPathComponent("mac-host-settings-\(name).json")
        if ProcessInfo.processInfo.environment["UPDATE_WEB_FIXTURES"] == "1" {
            try FileManager.default.createDirectory(at: Self.fixtures, withIntermediateDirectories: true)
            try data.write(to: file)
        }
        let stored = try Data(contentsOf: file)
        #expect(String(decoding: stored, as: UTF8.self) == String(decoding: data, as: UTF8.self),
                "\(file.lastPathComponent) is stale: UPDATE_WEB_FIXTURES=1 scripts/test.sh --filter RuntimeSettingsTests")
    }

    /// Before anything was read, every kind's row is there, checking.
    @Test func everyKindHasARowOnTheFirstRead() async throws {
        let (bridge, model) = bridge()
        #expect(model.runtimeKinds == nil)
        let answer = try await get(bridge)
        #expect(ids(answer) == ["runtime:container", "runtime:vm", "runtime:microvm"])
        #expect(row(answer, "runtime:microvm")?["help"] as? String == "Checking\u{2026}")
        let runtimes = try #require(answer["runtimes"] as? [String: Any])
        #expect(runtimes["kinds"] is NSNull)
        #expect(runtimes["os"] as? String == "macos")
        try checkFixture(answer, "unread")
    }

    /// Read: Already installed lists what is installed, autofills the path
    /// in use (Detected) and says what it is; a kind that can't run says why.
    @Test func alreadyInstalledShowsWhatIsInstalledAndThePathInUse() async throws {
        let backend = FixtureSpacesBackend()
        backend.fixtureBackends = ["managed", "lume", "vz"]
        backend.fixtureRuntimeSettings = ["runtime.container": "system", "runtime.vm": "builtin"]
        let (bridge, model) = bridge(backend)
        await model.readRuntimes()
        let answer = try await get(bridge)
        #expect(ids(answer) == ["runtime:container", "runtime-detected:container", "runtime-path:container",
                                "runtime-check:container", "runtime:vm", "runtime:microvm"])
        let path = try #require(row(answer, "runtime-path:container"))
        #expect(path["label"] as? String == "Detected")
        #expect(path["value"] as? String == "/usr/local/bin/docker")
        #expect(row(answer, "runtime-check:container")?["label"] as? String == "Docker 27.3 found")
        let detected = try #require(row(answer, "runtime-detected:container")?["options"] as? [[String: Any]])
        #expect(detected.first?["label"] as? String == "Docker \u{2014} /usr/local/bin/docker (27.3)")
        try checkFixture(answer, "read")
    }

    /// Each choice writes the per-kind setting the core names; a typed path
    /// is Custom (Reset to detected); an empty one goes back to detection.
    @Test func choicesWriteThePerKindSettings() async throws {
        let backend = FixtureSpacesBackend()
        backend.fixtureBackends = ["managed", "lume", "vz"]
        let (bridge, model) = bridge(backend)
        await model.readRuntimes()
        _ = try await bridge.handle("settings.choose", ["row": "runtime:microvm", "option": "system"])
        #expect(backend.fixtureRuntimeSettings["runtime.microvm"] == "system")
        _ = try await bridge.handle("settings.choose", ["row": "runtime:container", "option": "system"])
        _ = try await bridge.handle("settings.choose", ["row": "runtime-path:container", "option": "/Users/me/bin/docker"])
        #expect(backend.fixtureRuntimeSettings["runtime.container_path"] == "/Users/me/bin/docker")
        var answer = try await get(bridge)
        let path = try #require(row(answer, "runtime-path:container"))
        #expect(path["label"] as? String == "Custom")
        #expect(path["button"] as? String == "Reset to detected")
        #expect(row(answer, "runtime-check:container")?["kind"] as? String == "error")
        _ = try await bridge.handle("settings.choose", ["row": "runtime-path:container", "option": ""])
        #expect(backend.fixtureRuntimeSettings["runtime.container_path"] == nil)
        answer = try await get(bridge)
        #expect(row(answer, "runtime-path:container")?["value"] as? String == "/usr/local/bin/docker")
    }

    /// `runtimes.check` says what a typed path is; `runtimes.browse` is the
    /// file panel.
    @Test func aTypedPathIsCheckedAndBrowsed() async throws {
        let (bridge, _) = bridge()
        let check = try #require(try await bridge.handle("runtimes.check", ["kind": "container", "path": "/usr/local/bin/docker"]) as? [String: Any])
        #expect(check["tool"] as? String == "docker")
        #expect(check["version"] as? String == "27.3.1")
        let missing = try #require(try await bridge.handle("runtimes.check", ["kind": "container", "path": "/nope"]) as? [String: Any])
        #expect(missing["exists"] as? Bool == false)
        #expect(missing["expected"] as? [String] == ["Docker", "Podman"])
        bridge.pickRuntime = { _ in URL(fileURLWithPath: "/opt/homebrew/bin/podman") }
        let picked = try await bridge.handle("runtimes.browse", ["kind": "container"]) as? [String: Any]
        #expect(picked?["path"] as? String == "/opt/homebrew/bin/podman")
        bridge.pickRuntime = { _ in nil }
        #expect((try await bridge.handle("runtimes.browse", ["kind": "container"]) as? [String: Any])?["path"] is NSNull)
    }
}
