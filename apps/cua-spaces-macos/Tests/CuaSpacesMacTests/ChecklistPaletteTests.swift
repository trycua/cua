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
        let rows = m.machinePageRows
        #expect(rows.first?.current == true)
        #expect(rows.first?.id == "this-mac")
        let ids = rows.map(\.id)
        #expect(!ids.contains("local:aurora"))
        #expect(!ids.contains("direct:10.0.0.5:3211"))
        #expect(Set(ids).count == ids.count, "each computer once")
    }

    /// A model signed in, with the fixture account's devices read.
    private func signedInModel() async -> AppModel {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("cua-machines-\(UUID().uuidString)")
        let m = AppModel(backend: FixtureSpacesBackend(), keyvault: KeyvaultModel(client: nil),
                         onboarding: OnboardingModel(statePath: dir.appendingPathComponent("onboarding.json").path),
                         settingsPath: dir.appendingPathComponent("settings.json").path,
                         telemetry: FixtureTelemetry(), devices: FixtureDevices())
        await m.refresh()
        m.devices.signedIn = true
        await m.devices.refresh()
        return m
    }

    /// CUA-1510: the classic Machines page lists the devices that are only
    /// in the device list too (the New UI's rows: the core's page rows).
    @Test func machinesListDevicesThatAreOnlyInTheDeviceList() async {
        let m = await signedInModel()
        let rows = m.machinePageRows
        #expect(rows.first?.current == true)
        #expect(rows.first?.device == "dev_mac", "this Mac and its device are one row")
        let own = rows.filter(\.deviceOnly).map(\.name)
        #expect(own.contains("Work laptop"))
        #expect(own.contains("Old laptop"))
        #expect(rows.filter(\.deviceOnly).allSatisfy { $0.spaceId == nil && !$0.current })
        // The New UI gets the same rows from the bridge.
        let bridge = WebUIBridge(model: m, allowedOrigins: [])
        let answer = bridge.machines()["rows"] as? [[String: Any]]
        #expect(answer?.map { $0["id"] as? String } == rows.map(\.id))
        // And a device of your own is another machine for the checklist.
        if case let .observe(otherMachine, _, _) = m.checklistSeen {
            #expect(otherMachine)
        } else {
            Issue.record("not an observation")
        }
    }

    /// CUA-1617: a Mac whose owner stopped sharing reads "Not sharing" and
    /// "No Spaces" in the classic list, from the core's row (never a
    /// Space's "Suspended"), and the New UI gets the same words.
    @Test func aMachineThatStoppedSharingReadsTheSameInBothUIs() async throws {
        let input = try appMachinesPageInputFromJson(json: """
        {"hostOs": "macos", "now": 1800000000, "spaces": [
          {"id": "relay:b695", "name": "Dillons-MacBook-Pro-2.local", "os": "macos",
           "status": "suspended", "detail": "", "lastUsedAt": 0, "scene": "linux-terminal",
           "provider": "relay",
           "sdk": {"features": [], "spacesdVersion": "", "reachable": false,
                   "error": "unauthenticated: Dillons-MacBook-Pro-2.local stopped sharing: ask its owner"}}
        ], "devices": [], "hostnames": {}, "presence": {"b695": false}}
        """)
        let row = appMachinesPageRows(input: input)[1]
        #expect(row.status == "Not sharing")
        #expect(row.spacesLabel == "No Spaces")

        let m = await signedInModel()
        let rows = m.machinePageRows
        let bridge = WebUIBridge(model: m, allowedOrigins: [])
        let answer = bridge.machines()["rows"] as? [[String: Any]]
        #expect(answer?.map { $0["status"] as? String } == rows.map(\.status))
        #expect(answer?.map { $0["spacesLabel"] as? String } == rows.map(\.spacesLabel))
    }

    /// CUA-1509: the app ticks the checklist off itself, whichever UI is
    /// open, and the New UI reads and changes the same copy.
    @Test func bothUIsShareOneChecklist() async throws {
        let m = await signedInModel()
        // No view asked: the model followed what it shows.
        for _ in 0..<20 where !m.checklist.state.done.contains(.addMachine) {
            try await Task.sleep(for: .milliseconds(10))
        }
        #expect(m.checklist.state.done.contains(.addMachine))
        #expect(m.checklist.state.done.contains(.createSpace))
        let bridge = WebUIBridge(model: m, allowedOrigins: [])
        let got = try await bridge.handle("checklist.get", [:]) as? [String: Any]
        let words = ((got?["state"] as? [String: Any])?["done"] as? [String]) ?? []
        #expect(words.contains("add_machine") && words.contains("create_space"))
        #expect(m.checklist.view.progress == "\(words.count) of 5")
        // The New UI ticks a step: the classic UI's copy has it.
        _ = try await bridge.handle("checklist.send", ["action": ["type": "done", "step": "teleport"]])
        #expect(m.checklist.state.done.contains(.teleport))
        #expect(m.checklist.view.progress == "\(words.count + 1) of 5")
        // What the page kept itself before is merged once, without events.
        let telemetry = FixtureTelemetry()
        m.checklist.telemetry = telemetry
        let merged = try await bridge.handle("checklist.send", ["action": [
            "type": "merge", "state": ["done": ["share", "nope"], "dismissed": false, "shown": true],
        ]]) as? [String: Any]
        let after = ((merged?["state"] as? [String: Any])?["done"] as? [String]) ?? []
        #expect(after.contains("share") && !after.contains("nope"))
        #expect(m.checklist.state.shown)
        #expect(telemetry.recorded.isEmpty)
        // The page never sends what the app shows.
        await #expect(throws: WebUIBridge.Failure.self) {
            _ = try await bridge.handle("checklist.send", ["action": ["type": "observe", "otherMachine": true]])
        }
    }

    /// A Space's agent runs, read anywhere, tick "Run an agent".
    @Test func agentRunsSeenTickRunAnAgent() async throws {
        let m = await signedInModel()
        m.sawAgentRuns(1)
        for _ in 0..<20 where !m.checklist.state.done.contains(.runAgent) {
            try await Task.sleep(for: .milliseconds(10))
        }
        #expect(m.checklist.state.done.contains(.runAgent))
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
            AppPaletteEntry(group: "Go to", label: "Computers", hint: nil, keywords: "page"),
            AppPaletteEntry(group: "Spaces", label: "Stop builder", hint: "Running", keywords: nil),
        ]
        #expect(appPaletteRank(entries: entries, query: "") == [0, 1, 2])
        #expect(appPaletteRank(entries: entries, query: "comp") == [1])
        #expect(appPaletteRank(entries: entries, query: "stop run") == [2])
    }
}
