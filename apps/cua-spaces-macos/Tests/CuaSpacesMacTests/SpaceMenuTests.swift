// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpacesFFI
@testable import CuaSpacesMacKit
import Foundation
import SwiftUI
import Testing

/// The classic window's Space menus and picking several Spaces (CUA-2204):
/// the right-click menu is the core's for each kind of Space, the picked
/// Spaces become the core's confirm, ⌘A and Esc change the pick, and one
/// confirm deletes yours and leaves the others through the backend.
@MainActor
@Suite("Space menus and bulk delete or leave")
struct SpaceMenuTests {
    static func row(_ id: String, _ name: String, provider: String = "local", running: Bool = false,
                    hostName: String? = nil, sharing: AppSpaceSharingFacts? = nil) -> AppSpaceRow {
        AppSpaceRow(id: id, name: name, displayName: nil, provider: provider, spacesdVersion: "0.4.0",
                    features: ["desktop_stream"], addedAt: "2026-09-25T08:00:00Z", os: .linux,
                    osName: "Ubuntu", osPrettyName: nil, image: nil, imageDigest: nil, kind: .container,
                    arch: "arm64", reachable: running, error: nil, host: nil, hostName: hostName,
                    power: "stop", powerState: running ? "running" : "stopped", cloud: nil, cloudPlace: nil,
                    cloudDelete: nil, stopReason: nil, sharing: sharing)
    }

    static func facts(role: String, owner: String = "sam", editors: [String] = [], team: String? = nil)
        -> AppSpaceSharingFacts {
        AppSpaceSharingFacts(ownerId: owner, ownerEmail: "\(owner)@example.com",
                             ownerName: owner == "sam" ? "Sam Rivera" : nil, role: role, team: team,
                             teamName: team.map { _ in "QA team" }, editors: editors, viewers: [], teams: [])
    }

    /// Yours running and shared with Priya, yours stopped, Sam's shared with
    /// you, and a team's.
    static let rows: [AppSpaceRow] = [
        row("local:cua-builder", "cua-builder", running: true,
            sharing: facts(role: "owner", owner: "me", editors: ["priya@example.com"])),
        row("local:onb-new", "onb-new"),
        row("relay:sam/qa-shared", "qa-shared", provider: "relay", hostName: "Mac mini",
            sharing: facts(role: "editor")),
        row("relay:qa/nightly-e2e", "nightly-e2e", provider: "relay", hostName: "CI box",
            sharing: facts(role: "editor", team: "qa")),
    ]

    func makeModel(_ backend: FixtureSpacesBackend = FixtureSpacesBackend(rows: SpaceMenuTests.rows)) async
        -> AppModel {
        let m = ViewModelTests().makeModel(backend)
        m.settings.experiments.sharing = true
        await m.refresh()
        return m
    }

    func space(_ m: AppModel, _ id: String) throws -> AppSpace {
        try #require(m.spaces.first { $0.id == id })
    }

    // MARK: - The menu per kind of Space

    @Test func yoursSharedReadsManageSharingThenDeleteInRed() async throws {
        let m = await makeModel()
        let items = m.spaceMenu(try space(m, "local:cua-builder"))
        #expect(items.map(\.label) == ["Open", "Turn off", "Manage sharing\u{2026}", "Teleport an app\u{2026}",
                                       "Delete Space"])
        let delete = try #require(items.last)
        #expect(delete.id == .delete && delete.destructive && delete.separatorBefore)
        #expect(!items.contains { $0.id == .rename }, "no Rename until this app renames a Space")
    }

    @Test func yoursStoppedSaysShareAndGreysTeleport() async throws {
        let m = await makeModel()
        let items = m.spaceMenu(try space(m, "local:onb-new"))
        #expect(items.map(\.id) == [.open, .power, .share, .teleport, .delete])
        #expect(items.first { $0.id == .power }?.label == "Turn on")
        #expect(items.first { $0.id == .teleport }?.enabled == false, "Teleport needs a running desktop")
    }

    @Test func sharingOffLeavesShareOut() async throws {
        let m = await makeModel()
        m.settings.experiments.sharing = false
        #expect(!m.spaceMenu(try space(m, "local:onb-new")).contains { $0.id == .share })
    }

    @Test func someoneElsesOffersLeaveNotDelete() async throws {
        let m = await makeModel()
        let items = m.spaceMenu(try space(m, "relay:sam/qa-shared"))
        #expect(items.map(\.id) == [.open, .power, .teleport, .leave])
        let leave = try #require(items.last)
        #expect(leave.label == "Leave" && !leave.destructive && leave.separatorBefore)
    }

    @Test func aTeamsSpaceOffersLeave() async throws {
        let m = await makeModel()
        let s = try space(m, "relay:qa/nightly-e2e")
        #expect(m.menuInput(s).kind == .team && !m.menuInput(s).maker)
        #expect(m.spaceMenu(s).last?.id == .leave)
    }

    @Test func menuItemsRunTheDetailsOwnFlows() async throws {
        let m = await makeModel()
        let s = try space(m, "local:cua-builder")
        m.runMenuItem(.manageSharing, on: s)
        #expect(m.selection == .space(s.id))
        #expect(m.detailRequest == DetailRequest(spaceId: s.id, action: .share))
        m.runMenuItem(.teleport, on: s)
        #expect(m.detailRequest == DetailRequest(spaceId: s.id, action: .teleport))
        m.runMenuItem(.delete, on: s)
        let ask = try #require(m.bulkAsk)
        #expect(ask.view.title == "Delete cua-builder?" && ask.view.button == "Delete" && ask.view.destructive)
        #expect(ask.view.body.contains("cua-builder is running and will be turned off first."))
        #expect(ask.view.body.contains("priya loses access."))
    }

    // MARK: - Picking several

    @Test func selectAllPicksEverySpaceAndEscGoesBackToOne() async throws {
        let m = await makeModel()
        m.select("local:onb-new")
        m.selectAllSpaces()
        #expect(m.pickedSpaces.map(\.id).sorted() == Self.rows.map(\.id).sorted())
        #expect(m.selection == .space("local:onb-new"), "the Space last chosen stays the one")
        #expect(m.sidebarSelection.count == Self.rows.count)
        #expect(m.clearPickedSpaces())
        #expect(m.pickedSpaces.isEmpty && m.sidebarSelection == [.space("local:onb-new")])
        #expect(!m.clearPickedSpaces(), "Esc with one selected is not the list's")
    }

    @Test func theListsSelectionPicksSpacesAndDropsTheRest() async throws {
        let m = await makeModel()
        m.select("local:onb-new")
        // ⌘-click a second Space.
        m.setSidebarSelection([.space("local:onb-new"), .space("relay:sam/qa-shared")])
        #expect(m.pickedSpaces.count == 2 && m.selection == .space("relay:sam/qa-shared"))
        // The list's own Select All also selects the other rows: only the Spaces stay.
        m.setSidebarSelection(Set(Self.rows.map { MainSelection.space($0.id) }).union([.agents, .machines]))
        #expect(m.pickedSpaces.count == Self.rows.count)
        #expect(!m.sidebarSelection.contains(.agents))
        // ⌘-click Agents: Agents alone.
        m.setSidebarSelection(m.sidebarSelection.union([.agents]))
        #expect(m.selection == .agents && m.pickedSpaces.isEmpty)
        // A plain click on one Space: that one.
        m.setSidebarSelection([.space("local:cua-builder")])
        #expect(m.selection == .space("local:cua-builder") && m.pickedSpaces.isEmpty)
    }

    @Test func pickedSpacesBecomeTheConfirmsItems() async throws {
        let m = await makeModel()
        m.select("local:cua-builder")
        m.setSidebarSelection([.space("local:cua-builder"), .space("local:onb-new"), .space("relay:sam/qa-shared")])
        let picked = m.pickedSpaces
        let items = picked.map(m.bulkItem)
        #expect(items.map(\.id) == ["local:cua-builder", "local:onb-new", "relay:sam/qa-shared"])
        #expect(items.map(\.mine) == [true, true, false])
        #expect(items[0].running && !items[1].running)
        #expect(items[0].people == [AppBulkPerson(who: "priya@example.com", name: nil)])
        #expect(items[2].place == "Sam's Mac mini" && items[2].people.isEmpty)
        let view = try #require(m.bulkConfirm(picked))
        #expect(view.title == "Delete 2 Spaces and leave 1?")
        #expect(view.button == "Delete and leave" && view.destructive)
        #expect(view.groups.map(\.title) == ["Delete", "Leave"])
        #expect(view.body.hasSuffix("qa-shared stays on Sam's Mac mini."))
        #expect(view.frees == nil, "no sizes on disk here yet")
        // Only someone else's: plain Leave.
        let leave = try #require(m.bulkConfirm([try space(m, "relay:sam/qa-shared"), try space(m, "relay:qa/nightly-e2e")]))
        #expect(leave.title == "Leave 2 Spaces?" && leave.button == "Leave" && !leave.destructive)
    }

    @Test func pickedPowerTurnsOffWhileAnyRuns() async throws {
        let m = await makeModel()
        let both = [try space(m, "local:cua-builder"), try space(m, "local:onb-new")]
        #expect(m.pickedPower(both)?.label == "Turn off" && m.pickedPower(both)?.on == false)
        #expect(m.pickedPower([try space(m, "local:onb-new")])?.label == "Turn on")
    }

    // MARK: - Running it

    @Test func oneConfirmDeletesYoursAndLeavesTheRestThroughTheBackend() async throws {
        let backend = FixtureSpacesBackend(rows: Self.rows)
        let m = await makeModel(backend)
        m.select("local:cua-builder")
        m.selectAllSpaces()
        m.askRemove(m.pickedSpaces)
        let ask = try #require(m.bulkAsk)
        #expect(ask.view.deleteIds == ["local:cua-builder", "local:onb-new"])
        // In the sidebar's order.
        let theirs = m.pickedSpaces.filter { $0.id.hasPrefix("relay:") }
        #expect(ask.view.leaveIds == theirs.map(\.id))
        await m.confirmBulk()
        #expect(m.bulkAsk == nil && m.bulkRunItems == nil && m.bulkProgressText == nil)
        // Every id of yours reached the backend's delete exactly once.
        #expect(backend.removed.sorted() == ["local:cua-builder", "local:onb-new"])
        #expect(!m.spaces.contains { ask.view.deleteIds.contains($0.id) })
        // The fixture cannot leave (no relay): those stay, and the toast says so.
        let toast = try #require(m.bulkToast)
        #expect(toast.tone == .partial)
        #expect(toast.text == "Deleted 2 Spaces. Couldn't leave \(theirs[0].name) and \(theirs[1].name).")
        #expect(m.pickedSpaces.isEmpty)
    }

    @Test func theRunCountsThrough() async throws {
        let backend = FixtureSpacesBackend(rows: Self.rows)
        backend.holdRemoves = true
        let m = await makeModel(backend)
        m.select("local:cua-builder")
        m.askRemove([try space(m, "local:cua-builder"), try space(m, "local:onb-new")])
        let run = Task { await m.confirmBulk() }
        for _ in 0..<200 where !(m.isDeleting("local:cua-builder") && m.isDeleting("local:onb-new")) {
            try await Task.sleep(for: .milliseconds(10))
        }
        #expect(m.bulkProgressText == "Deleting 1 of 2\u{2026}")
        #expect(m.isDeleting("local:cua-builder") && m.isDeleting("local:onb-new"))
        backend.releaseRemove()
        await run.value
        #expect(m.bulkToast?.text == "Deleted 2 Spaces.")
    }
}

/// The new classic-window surfaces on fixtures: the detail with several
/// Spaces picked, and the confirm for deleting yours and leaving Sam's.
extension SnapshotTests {

}
