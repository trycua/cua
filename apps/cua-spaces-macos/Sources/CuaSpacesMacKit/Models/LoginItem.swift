// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSpacesFFI
import Foundation
import ServiceManagement

/// The app as a login item ("Launch Cua Spaces at login"). Everything that
/// registers, unregisters or reads it goes through this protocol, so tests
/// and fixtures run on `FixtureLoginItem` and never touch the Mac's real
/// login items.
public protocol LoginItemControlling: AnyObject, Sendable {
    /// What the system reports now.
    func status() -> AppLoginItemStatus
    /// Registers the app (it opens at login, once allowed).
    func register() throws
    /// Unregisters it.
    func unregister() throws
    /// Opens System Settings, Login Items.
    func openSystemSettings()
}

/// `SMAppService.mainApp`: this app itself as a login item. Its daemon
/// starts with it (`DaemonSupervisor`), so the Spaces this Mac provides,
/// its persistent agents and Cua Volume come back after a restart.
public final class MainAppLoginItem: LoginItemControlling, @unchecked Sendable {
    public init() {}

    public func status() -> AppLoginItemStatus { Self.map(SMAppService.mainApp.status) }
    public func register() throws { try SMAppService.mainApp.register() }
    public func unregister() throws { try SMAppService.mainApp.unregister() }
    public func openSystemSettings() { SMAppService.openSystemSettingsLoginItems() }

    static func map(_ s: SMAppService.Status) -> AppLoginItemStatus {
        switch s {
        case .enabled: return .enabled
        case .notRegistered: return .notRegistered
        case .requiresApproval: return .requiresApproval
        case .notFound: return .notFound
        @unknown default: return .notFound
        }
    }
}

/// An in-memory login item (fixtures, tests, captures). `approval` makes a
/// register wait for approval, as macOS does when the user turned the app
/// off in System Settings before.
public final class FixtureLoginItem: LoginItemControlling, @unchecked Sendable {
    public var current: AppLoginItemStatus
    public var approval = false
    public var failure: String?
    /// `register`, `unregister` and `open-settings`, in order.
    public private(set) var calls: [String] = []

    public init(_ status: AppLoginItemStatus = .notRegistered) { current = status }

    public func status() -> AppLoginItemStatus { current }

    public func register() throws {
        calls.append("register")
        if let failure { throw AgentsToolError(message: failure) }
        current = approval ? .requiresApproval : .enabled
    }

    public func unregister() throws {
        calls.append("unregister")
        if let failure { throw AgentsToolError(message: failure) }
        current = .notRegistered
    }

    public func openSystemSettings() { calls.append("open-settings") }
}

/// Whether this launch is the system opening the app at login: the open
/// application event carries `keyAELaunchedAsLogInItem`. Then the app
/// starts in the menu bar (and the notch) without its main window, as it
/// stays after the window is closed.
public enum LoginLaunch {
    public static func isLoginLaunch(_ event: NSAppleEventDescriptor?) -> Bool {
        guard let event, event.eventClass == AEEventClass(kCoreEventClass),
              event.eventID == AEEventID(kAEOpenApplication) else { return false }
        return event.paramDescriptor(forKeyword: AEKeyword(keyAEPropData))?.enumCodeValue
            == OSType(keyAELaunchedAsLogInItem)
    }
}
