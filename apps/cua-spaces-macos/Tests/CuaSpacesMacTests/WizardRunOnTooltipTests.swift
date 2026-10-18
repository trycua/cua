// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesMacKit
import CuaSpacesFFI
import Testing

/// New Space's "Run on" tooltip says what This Mac runs the Space on, for
/// every system (in 0.7.2 for Linux and Windows too; it regressed to macOS
/// only). The menu itself now carries the chosen place's line
/// (`NewSpaceWizardView.runOnHelp`), so it no longer depends on which item
/// SwiftUI's menu kept the tooltip of.
@Suite("New Space Run on tooltip")
@MainActor
struct WizardRunOnTooltipTests {
    @Test func theTooltipFollowsTheSystem() async throws {
        let m = ViewModelTests().makeModel(FixtureSpacesBackend(), kv: nil, host: FixtureHost(),
                                           account: FixtureAccount(), agents: FixtureAgentSetup(),
                                           loginItem: FixtureLoginItem())
        m.onboarding.finish()
        await m.refresh()
        await m.openNewSpace()
        var seen: [AppSpaceOs: String] = [:]
        // macOS first, then the others: each one's own line, never the last one's.
        for os in [AppSpaceOs.macos, .linux, .windows, .macos] {
            m.wizard.send(.chooseOs(os: os))
            m.wizard.send(.choosePlacement(on: "local"))
            let v = m.wizard.view
            #expect(v.placementId == "local")
            seen[os] = NewSpaceWizardView.runOnHelp(v)
        }
        #expect(seen[.linux] == "Container on this Mac (gVisor when installed)")
        #expect(seen[.windows] == "QEMU virtual machine on this Mac")
        #expect(seen[.macos] == "Lume virtual machine (Apple silicon)")
    }
}
