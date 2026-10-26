// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesMacKit
import AppKit
import CuaSDK
import CuaSpacesFFI
import Foundation
import Testing

/// `spaces.rename {spaceId, name} -> {}` on the Mac: the SDK renames the
/// Space (its id stays the same), the list shows the new name, and a
/// refusal comes back in plain words.
@Suite("Rename a Space")
@MainActor
struct RenameTests {
    init() { _ = NSApplication.shared }

    func bridge() async -> (WebUIBridge, AppModel, FixtureSpacesBackend) {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("cua-rename-\(UUID().uuidString)")
        let backend = FixtureSpacesBackend()
        let model = AppModel(backend: backend, keyvault: KeyvaultModel(client: nil),
                             onboarding: OnboardingModel(statePath: dir.appendingPathComponent("onboarding.json").path),
                             settingsPath: dir.appendingPathComponent("settings.json").path,
                             telemetry: FixtureTelemetry())
        await model.refresh()
        let bridge = WebUIBridge(model: model, allowedOrigins: [])
        bridge.actions = WebUIActions(openSpace: { _ in }, openMain: {})
        return (bridge, model, backend)
    }

    @Test func renamesKeepsTheIdAndListsTheNewName() async throws {
        let (bridge, model, backend) = await bridge()
        let answer = try await bridge.handle("spaces.rename", ["spaceId": "local:aurora", "name": "design-review"])
        #expect((answer as? [String: Any])?.isEmpty == true)
        #expect(backend.renamed.map(\.id) == ["local:aurora"])
        #expect(backend.renamed.map(\.name) == ["design-review"])
        let space = try #require(model.spaces.first { $0.id == "local:aurora" })
        #expect(space.name == "design-review")
    }

    @Test func aRefusalIsInPlainWords() async throws {
        let (bridge, model, backend) = await bridge()
        // The relay's refusal, as the SDK raises it (a flat error: its kind in front).
        backend.renameError = CuaError.PermissionDenied(
            message: "permission denied: studio is shared with you. Only its owner can rename it.")
        do {
            _ = try await bridge.handle("spaces.rename", ["spaceId": "local:aurora", "name": "mine"])
            Issue.record("expected a refusal")
        } catch {
            // What the page reads (`Failure(code: "failed", message: words(error))`).
            #expect(LiveSpacesBackend.words(error) == "studio is shared with you. Only its owner can rename it.")
        }
        #expect(model.spaces.first { $0.id == "local:aurora" }?.name != "mine")
        do {
            _ = try await bridge.handle("spaces.rename", ["spaceId": "local:nope", "name": "x"])
            Issue.record("expected not found")
        } catch let f as WebUIBridge.Failure {
            #expect(f.code == "not_found")
        }
        do {
            _ = try await bridge.handle("spaces.rename", ["spaceId": "local:aurora"])
            Issue.record("expected bad args")
        } catch let f as WebUIBridge.Failure {
            #expect(f.code == "bad_args")
        }
    }
}
