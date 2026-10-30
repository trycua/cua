// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpacesFFI
@testable import CuaSpacesMacKit
import Foundation
import Testing

/// Bulk delete and leave (`spaces.deleteMany`, `spaces.leave`): every row
/// says Deleting until its own result comes back, one failure never stops
/// the rest, and the bridge answers one result per id in order. A Space
/// shared with you carries its sharing facts to the core.
@MainActor
@Suite("Bulk delete and leave")
struct BulkDeleteTests {
    func makeModel(_ backend: SpacesBackend) -> AppModel {
        ViewModelTests().makeModel(backend)
    }

    static func sharedRow() -> AppSpaceRow {
        AppSpaceRow(id: "relay:space-sam-qa", name: "qa-shared", displayName: nil, provider: "relay", spacesdVersion: "0.4.0",
                    features: [], addedAt: nil, os: .macos, osName: nil, osPrettyName: nil, image: nil,
                    imageDigest: nil, kind: nil, arch: nil, reachable: true, error: nil, host: nil,
                    hostName: "Mac mini", power: "stop", powerState: "running", cloud: nil, cloudPlace: nil,
                    cloudDelete: nil, stopReason: nil,
                    sharing: AppSpaceSharingFacts(ownerId: "sam", ownerEmail: "sam@example.com",
                                                  ownerName: "Sam Rivera", role: "editor", team: nil,
                                                  teamName: nil, editors: [], viewers: [], teams: []))
    }

    @Test func deletingSeveralShowsEachDeletingAndAnswersOncePerId() async throws {
        let backend = FixtureSpacesBackend()
        backend.holdRemoves = true
        let model = makeModel(backend)
        await model.refresh()
        let ids = model.spaces.filter { $0.sdk != nil }.prefix(2).map(\.id)
        #expect(ids.count == 2)
        let run = Task { await model.bulk(ids: ids + ["local:nowhere"], leave: false) }
        for _ in 0..<200 where !ids.allSatisfy({ model.isDeleting($0) }) { try await Task.sleep(for: .milliseconds(10)) }
        #expect(ids.allSatisfy { model.isDeleting($0) }, "every row says Deleting while it goes")
        backend.releaseRemove()
        let results = await run.value
        #expect(results.map(\.id) == ids + ["local:nowhere"])
        #expect(results.prefix(2).allSatisfy { $0.ok })
        #expect(results.last == SpaceBulkResult(id: "local:nowhere", ok: false, error: "No Space local:nowhere."))
        #expect(Set(backend.removed) == Set(ids))
        #expect(!model.spaces.contains { ids.contains($0.id) })
    }

    @Test func oneFailureRestoresItsRowAndTheRestStillGo() async throws {
        let backend = FixtureSpacesBackend()
        let model = makeModel(backend)
        await model.refresh()
        let ids = model.spaces.filter { $0.sdk != nil }.prefix(2).map(\.id)
        // The first remove fails, the second goes.
        backend.removeError = "lume is not running"
        let results = await model.bulk(ids: ids, leave: false)
        #expect(results.filter { $0.ok }.count == 1)
        #expect(results.contains { !$0.ok && $0.error == "lume is not running" })
        #expect(ids.allSatisfy { !model.isDeleting($0) })
    }

    @Test func aSpaceSharedWithYouSaysWhoseItIsAndLeavingNeedsTheLiveApp() async throws {
        let backend = FixtureSpacesBackend(rows: [Self.sharedRow()])
        let model = makeModel(backend)
        await model.refresh()
        let space = try #require(model.spaces.first { $0.id == "relay:space-sam-qa" })
        let access = try #require(space.access)
        #expect(access.kind == .withMe && access.role == .editor)
        #expect(access.place == "Sam's Mac mini" && access.ownerName == "Sam Rivera")
        #expect(access.canLeave && !access.canDelete && !access.canShare)
        // The fixture has no relay: refused, the row stays.
        let results = await model.bulk(ids: [space.id], leave: true)
        #expect(results == [SpaceBulkResult(id: space.id, ok: false, error: "Leaving a Space isn't available here.")])
        #expect(model.spaces.contains { $0.id == space.id } && !model.isDeleting(space.id))
    }
}
