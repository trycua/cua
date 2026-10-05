// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
import CuaSpacesStreaming
@testable import CuaSpacesMacKit
import Foundation
import Testing
import SwiftUI

/// Only the roster is scripted. No service, account, or desktop is contacted.
private final class DiscoveryBackend: SpacesBackend, @unchecked Sendable {
    var result: Result<[AppSpaceRow], Error> = .success([])
    func rows() async throws -> [AppSpaceRow] { try result.get() }
    func create(_ args: AppCreateSpaceArgs, createId: String, progress: @escaping @Sendable (SpaceCreateProgress) -> Void) async throws -> String { fatalError("unexpected create") }
    func cancelCreate(createId: String) async throws {}
    func add(url: String, token: String?, name: String?) async throws {}
    func remove(id: String, removeOnly: Bool) async throws {}
    func setPower(id: String, on: Bool) async throws {}
    func streamProvider(id: String) async throws -> SpaceStreamSourceProviding {
        try await FixtureSpacesBackend().streamProvider(id: id)
    }
    func localBackends() async -> [String]? { nil }
    func localStorage() async -> LocalStorage? { nil }
    func cloudAvailable() async -> Bool { false }
    func teleportContext(id: String) async throws -> (CuaSpacesFFI.Teleport, CuaSDK.Space)? { nil }
    func teleportHandle() -> CuaSpacesFFI.Teleport? { nil }
    func sendFiles(id: String, paths: [String]) async throws -> [AppSentFileInfo] { [] }
    func agentRuns(id: String) async throws -> [AppSpaceAgentRun] { [] }
}

@MainActor
@Suite("Discovery errors", .serialized)
struct DiscoveryTests {
    @Test func coldFailureIsSafeAndRecoveryClearsOnlyRosterError() async {
        let backend = DiscoveryBackend()
        backend.result = .failure(CuaError.Internal(message: "https://user:secret@relay.invalid/?token=private"))
        let model = ViewModelTests().makeModel(backend)
        model.show(error: "Unrelated action failed")
        await model.refresh()
        #expect(model.rosterError == "Could not load Spaces. Try refreshing again.")
        #expect(!model.loaded)
        backend.result = .success([])
        await model.refresh()
        #expect(model.rosterError == nil)
        #expect(model.loaded)
        #expect(model.banner == "Unrelated action failed")
    }

    @Test func warmFailureRetainsRowsAndEmptySuccessRemovesThem() async {
        let backend = DiscoveryBackend()
        backend.result = .success(FixtureSpacesBackend.sample)
        let model = ViewModelTests().makeModel(backend)
        await model.refresh()
        let before = model.spaces.map(\.id)
        #expect(!before.isEmpty)
        backend.result = .failure(CuaError.Internal(message: "transport failed"))
        await model.refresh()
        #expect(model.spaces.map(\.id) == before)
        #expect(model.rosterError == "Could not refresh Spaces. Previously loaded rows may be out of date.")
        backend.result = .success([])
        await model.refresh()
        #expect(!model.spaces.contains { $0.id == "local:aurora" || $0.id == "cloud:builder" })
        #expect(model.rosterError == nil)
    }

    @Test func warmFailureSidebarRetainsRows() async throws {
        let snapshots = SnapshotTests()
        let backend = DiscoveryBackend()
        backend.result = .success(FixtureSpacesBackend.sample)
        let model = ViewModelTests().makeModel(backend)
        await model.refresh()
        backend.result = .failure(CuaError.Internal(message: "fixture failure"))
        await model.refresh()
        try snapshots.assertSnapshot(Sidebar(model: model).frame(width: 260),
            "discovery-warm-sidebar", size: CGSize(width: 260, height: 520))
    }
}
