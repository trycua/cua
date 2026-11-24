// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Testing
@testable import CuaSpacesMacKit

struct LaunchAgentRegistrationTests {
    typealias R = LaunchAgentRegistration

    @Test func picksTheStepFromBothRecords() {
        #expect(R.step(status: .enabled, loaded: true) == .none)
        #expect(R.step(status: .enabled, loaded: false) == .reregister)
        #expect(R.step(status: .notRegistered, loaded: false) == .register)
        #expect(R.step(status: .notFound, loaded: false) == .register)
        #expect(R.step(status: .requiresApproval, loaded: false) == .needsApproval)
    }

    /// The state after `launchctl bootout`: macOS still says enabled, but
    /// launchd has no job. Registering alone does nothing there, so it
    /// unregisters first, and the job is loaded again.
    @Test func reregistersAnEnabledItemLaunchdLost() throws {
        let item = FixtureLoginItem(.enabled)
        var loaded = false
        let step = try R.ensureLoaded(item, isLoaded: {
            if item.calls == ["unregister", "register"] { loaded = true }
            return loaded
        }, settle: {})
        #expect(step == .reregister)
        #expect(item.calls == ["unregister", "register"])
        #expect(item.status() == .enabled)
        #expect(loaded)
    }

    @Test func leavesALoadedItemAlone() throws {
        let item = FixtureLoginItem(.enabled)
        #expect(try R.ensureLoaded(item, isLoaded: { true }, settle: {}) == .none)
        #expect(item.calls.isEmpty)
    }

    @Test func registersANewItemOnce() throws {
        let item = FixtureLoginItem(.notRegistered)
        #expect(try R.ensureLoaded(item, isLoaded: { item.status() == .enabled }, settle: {}) == .register)
        #expect(item.calls == ["register"])
    }

    @Test func neverReregistersWhileMacOSWaitsForApproval() throws {
        let item = FixtureLoginItem(.requiresApproval)
        #expect(try R.ensureLoaded(item, isLoaded: { false }, settle: {}) == .needsApproval)
        #expect(item.calls.isEmpty)
    }

    @Test func reportsARegisterThatFails() {
        let item = FixtureLoginItem(.notRegistered)
        item.failure = "Operation not permitted"
        #expect(throws: (any Error).self) { try R.ensureLoaded(item, isLoaded: { false }, settle: {}) }
    }
}
