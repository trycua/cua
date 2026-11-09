// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpacesFFI
import Darwin
import Foundation
import ServiceManagement

/// One of this app's `SMAppService.agent` login items (the host service,
/// Cua Driver) as a `LoginItemControlling`, so what registers it can run
/// on `FixtureLoginItem` in tests.
public final class LaunchAgentItem: LoginItemControlling, @unchecked Sendable {
    let service: SMAppService

    public init(plistName: String) { service = SMAppService.agent(plistName: plistName) }

    public func status() -> AppLoginItemStatus { MainAppLoginItem.map(service.status) }
    public func register() throws { try service.register() }
    public func unregister() throws { try service.unregister() }
    public func openSystemSettings() { SMAppService.openSystemSettingsLoginItems() }
}

/// Registers a launch agent login item so launchd really has its job.
///
/// macOS keeps two records of an `SMAppService.agent`: Background Task
/// Management's ("enabled") and launchd's loaded job. They can disagree:
/// `launchctl bootout gui/<uid>/<label>`, a crash while the bundle was
/// being replaced, or a bundle swapped under a running item unload the job
/// while BTM still says `.enabled`. `register()` on an enabled item does
/// nothing, so the item stays unloaded until logout, and `launchctl
/// kickstart` fails. Unregistering and registering again makes BTM load
/// it again, without a logout and without touching BTM's database.
public enum LaunchAgentRegistration {
    public enum Step: Equatable, Sendable {
        /// Registered and loaded: nothing to do.
        case none
        /// Not registered (or not found): register it.
        case register
        /// BTM says enabled but launchd hasn't loaded the job: unregister,
        /// then register.
        case reregister
        /// macOS holds it back until the person allows it in System
        /// Settings; registering again won't change that.
        case needsApproval
    }

    public static func step(status: AppLoginItemStatus, loaded: Bool) -> Step {
        switch status {
        case .requiresApproval: return .needsApproval
        case .enabled: return loaded ? .none : .reregister
        case .notRegistered, .notFound: return .register
        }
    }

    /// Takes `step` on `item`, then gives launchd up to `settleTries`
    /// checks of `isLoaded` (with `settle` between them) to load the job.
    /// Returns the step it took. Throws what register or unregister threw;
    /// an unregister that fails but leaves the item unregistered is fine.
    @discardableResult
    public static func ensureLoaded(_ item: LoginItemControlling,
                                    isLoaded: () -> Bool,
                                    settleTries: Int = 10,
                                    settle: () -> Void = { usleep(200_000) }) throws -> Step {
        let step = step(status: item.status(), loaded: isLoaded())
        switch step {
        case .none, .needsApproval:
            return step
        case .reregister:
            do { try item.unregister() } catch {
                let now = item.status()
                if now != .notRegistered, now != .notFound { throw error }
            }
            try item.register()
        case .register:
            try item.register()
        }
        for _ in 0..<settleTries {
            if isLoaded() { break }
            settle()
        }
        return step
    }

    /// Whether launchd has `gui/<uid>/<label>` loaded (`launchctl print`
    /// succeeds), running or not. Blocks: call off the main thread.
    public static func isLoaded(label: String) -> Bool {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/bin/launchctl")
        p.arguments = ["print", "gui/\(getuid())/\(label)"]
        p.standardOutput = FileHandle.nullDevice
        p.standardError = FileHandle.nullDevice
        guard (try? p.run()) != nil else { return false }
        p.waitUntilExit()
        return p.terminationStatus == 0
    }
}
