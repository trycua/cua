// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpacesFFI
@testable import CuaSpacesMacKit
import Foundation
import SwiftUI
import Testing

/// The Machines page's rows, ⌘1–4 and the command palette: the core's
/// decisions, the React UI's keys.
@MainActor
struct MachinesPaletteTests {
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

    /// A machine sharing only its own desktop reads "Desktop shared", not "No Spaces".
    @Test func aMachineSharingOnlyItsDesktopReadsDesktopShared() throws {
        let input = try appMachinesPageInputFromJson(json: """
        {"hostOs": "macos", "now": 1800000000, "spaces": [
          {"id": "relay:solo", "name": "solo", "os": "macos", "status": "running", "detail": "",
           "lastUsedAt": 0, "scene": "linux-terminal", "provider": "relay",
           "sdk": {"features": ["desktop_stream"], "spacesdVersion": "", "reachable": true}}
        ], "devices": [], "hostnames": {}, "presence": {}}
        """)
        let row = appMachinesPageRows(input: input)[1]
        #expect(row.spaces == 0)
        #expect(row.spacesLabel == "Desktop shared")
    }

    /// A machine sharing its desktop and providing a Space reads "Desktop · 1 Space".
    @Test func aMachineSharingItsDesktopAndSpacesReadsDesktopAndCount() throws {
        let input = try appMachinesPageInputFromJson(json: """
        {"hostOs": "macos", "now": 1800000000, "spaces": [
          {"id": "relay:both", "name": "both", "os": "macos", "status": "running", "detail": "",
           "lastUsedAt": 0, "scene": "linux-terminal", "provider": "relay",
           "sdk": {"features": ["desktop_stream"], "spacesdVersion": "", "reachable": true}},
          {"id": "relay:both/space-1", "name": "s1", "os": "linux", "status": "running", "detail": "",
           "lastUsedAt": 0, "scene": "linux-terminal", "provider": "relay", "host": "both"}
        ], "devices": [], "hostnames": {}, "presence": {}}
        """)
        let row = appMachinesPageRows(input: input)[1]
        #expect(row.spaces == 1)
        #expect(row.spacesLabel == "Desktop · 1 Space")
    }

    /// A machine providing only Spaces reads the plain count.
    @Test func aMachineProvidingOnlySpacesReadsTheCount() throws {
        let input = try appMachinesPageInputFromJson(json: """
        {"hostOs": "macos", "now": 1800000000, "spaces": [
          {"id": "relay:box/space-1", "name": "s1", "os": "linux", "status": "running", "detail": "",
           "lastUsedAt": 0, "scene": "linux-terminal", "provider": "relay", "host": "box"},
          {"id": "relay:box/space-2", "name": "s2", "os": "linux", "status": "running", "detail": "",
           "lastUsedAt": 0, "scene": "linux-terminal", "provider": "relay", "host": "box"}
        ], "devices": [], "hostnames": {}, "presence": {}}
        """)
        let row = appMachinesPageRows(input: input)[1]
        #expect(row.spaces == 2)
        #expect(row.spacesLabel == "2 Spaces")
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
