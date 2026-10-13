// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpacesFFI
@testable import CuaSpacesMacKit
import Foundation
import SwiftUI
import Testing

/// The getting-started checklist, the Machines page's rows, ⌘1–4 and the
/// command palette: the core's decisions, the React UI's keys.
@MainActor
struct ChecklistPaletteTests {
    private func steps(_ t: FixtureTelemetry) -> [String] {
        t.recorded.compactMap { if case let .step(step, _) = $0 { return step } else { return nil } }
    }

    @Test func checklistStepsCountOnceAndAreKeptBetweenLaunches() throws {
        let path = FileManager.default.temporaryDirectory
            .appendingPathComponent("cua-checklist-\(UUID().uuidString)/checklist.json").path
        let telemetry = FixtureTelemetry()
        let checklist = ChecklistModel(path: path)
        checklist.telemetry = telemetry
        #expect(checklist.view.visible)
        #expect(checklist.view.progress == "0 of 5")

        checklist.send(.shown)
        checklist.send(.shown)
        checklist.done(.share)
        checklist.done(.share)
        checklist.send(.observe(otherMachine: false, hasSpace: true, agentRan: false))
        #expect(steps(telemetry) == ["checklist_shown", "checklist_share", "checklist_create_space"])
        #expect(checklist.view.progress == "2 of 5")

        let again = ChecklistModel(path: path)
        #expect(again.state.done == [.createSpace, .share])
        #expect(again.state.shown)

        again.send(.dismiss)
        #expect(!again.view.visible)
        #expect(!ChecklistModel(path: path).view.visible)
    }

    @Test func theLastStepCompletesIt() {
        let telemetry = FixtureTelemetry()
        let checklist = ChecklistModel(path: nil)
        checklist.telemetry = telemetry
        for step in [AppChecklistStep.addMachine, .createSpace, .teleport, .share, .runAgent] { checklist.done(step) }
        #expect(steps(telemetry).last == "checklist_completed")
        #expect(checklist.view.complete)
        #expect(checklist.view.title == "You're set up")
    }

    @Test func theAppTicksOffWhatItShows() async {
        let m = ViewModelTests().makeModel()
        await m.refresh()
        if case let .observe(otherMachine, hasSpace, agentRan) = m.checklistSeen {
            #expect(hasSpace, "the fixtures have a running Space")
            #expect(!otherMachine)
            #expect(!agentRan)
        } else {
            Issue.record("not an observation")
        }
        m.checklist.send(m.checklistSeen)
        #expect(m.checklist.state.done == [.createSpace])
    }

    @Test func machinesListThisMachineAndNoSpaces() async {
        let m = ViewModelTests().makeModel()
        await m.refresh()
        let ids = m.machineRows.map(\.id)
        #expect(!ids.contains("local:aurora"))
        #expect(!ids.contains("direct:10.0.0.5:3211"))
        #expect(ids.allSatisfy { id in id.hasPrefix("relay:") || m.spaces.first { $0.id == id }.map { m.detail($0).isHost } == true })
    }

    @Test func checklistButtonsGoWhereTheStepIsDone() async {
        let m = ViewModelTests().makeModel()
        await m.refresh()
        await m.runChecklistStep(.runAgent)
        #expect(m.selection == .agents)
        m.openGettingStarted()
        #expect(m.selection == .gettingStarted)
    }

    @Test func navigationKeysAreTheSharedOnes() {
        #expect(SharedShortcuts.shortcut("commandPalette.toggle") == KeyboardShortcut("k", modifiers: .command))
        #expect(SharedShortcuts.shortcut("nav.machines") == KeyboardShortcut("2", modifiers: .command))
        #expect(SharedShortcuts.label("nav.keyvault") == "⌘4")
        #expect(SharedShortcuts.label("theme.cycle") == "⌘⇧L")
        #expect(SharedShortcuts.shortcut("no.such.command") == nil)

        let m = ViewModelTests().makeModel()
        m.go("nav.machines")
        #expect(m.selection == .machines)
        m.go("nav.agents")
        #expect(m.selection == .agents)
    }

    @Test func thePaletteRanksWithTheCore() {
        let entries = [
            AppPaletteEntry(group: "Actions", label: "New Space", hint: nil, keywords: "create add"),
            AppPaletteEntry(group: "Go to", label: "Machines", hint: nil, keywords: "page"),
            AppPaletteEntry(group: "Spaces", label: "Stop builder", hint: "Running", keywords: nil),
        ]
        #expect(appPaletteRank(entries: entries, query: "") == [0, 1, 2])
        #expect(appPaletteRank(entries: entries, query: "mach") == [1])
        #expect(appPaletteRank(entries: entries, query: "stop run") == [2])
    }
}
