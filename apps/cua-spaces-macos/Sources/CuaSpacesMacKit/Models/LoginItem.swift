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

/// The host service's login item: this app's own executable
/// (`CuaSpacesMac --service-helper run`, so its permissions are the app's) run by
/// `SMAppService.agent` from Contents/Library/LaunchAgents/<label>.plist
/// (com.trycua.spaces.helper.plist for the released app). cua-host (the
/// `app-helper` runner, in this app's SDK and in the `cua` CLI) drives it
/// through `CuaSpacesMac --service-helper register|unregister|status`,
/// because only this bundle's code may register it; it writes what the
/// helper runs (<CuaHome>/helper/service.json, ~/.cua/helper/service.json
/// for the released app) and starts it with launchctl.
///
/// Each channel has its own helper, so Cua Spaces Local runs beside the
/// released app without sharing its login item, launchd label or spec:
/// `scripts/build-app.sh` stamps `CuaHelperLabel` and `CuaHome` (and names
/// the plist after the label) into the bundle, and this reads them. A bundle
/// without them is the released app and keeps the names it always had.
public enum HelperService {
    /// The released app's launchd label, and the default when the bundle
    /// has no `CuaHelperLabel` (`CuaChannel::helper_label` in cua-home).
    static let defaultLabel = "com.trycua.spaces.helper"
    /// The default Cua home, below the user's home, when the bundle has no
    /// `CuaHome` (`CuaChannel::home_dir` in cua-home).
    static let defaultHome = ".cua"

    /// The launchd label of this bundle's helper: `CuaHelperLabel`, else the
    /// released app's.
    public static func label(info: [String: Any]?) -> String {
        stamped(info, "CuaHelperLabel") ?? defaultLabel
    }

    /// The plist in Contents/Library/LaunchAgents: `<label>.plist`.
    public static func plistName(info: [String: Any]?) -> String {
        label(info: info) + ".plist"
    }

    /// Where cua-host writes what the helper runs: `<CuaHome>/helper/
    /// service.json` (`CuaHome` is `~/...` or absolute), else
    /// `~/.cua/helper/service.json`. `nil` for a bundle that names another
    /// channel (`CuaChannel` other than `stable`) without a usable home: it
    /// must never fall back to the released app's file, so it runs nothing.
    public static func specPath(info: [String: Any]?, home: String) -> String? {
        let base: String
        if let stampedHome = stamped(info, "CuaHome") {
            if stampedHome == "~" {
                base = home
            } else if stampedHome.hasPrefix("~/") {
                base = home + "/" + String(stampedHome.dropFirst(2))
            } else if stampedHome.hasPrefix("/") {
                base = stampedHome
            } else {
                return nil
            }
        } else if let channel = stamped(info, "CuaChannel"), channel.lowercased() != "stable" {
            return nil
        } else {
            base = home + "/" + defaultHome
        }
        return (base as NSString).appendingPathComponent("helper/service.json")
    }

    /// Where the host service's executable lives, relative to the app's
    /// `Contents`: `Helpers/cua-spacesd`, never `MacOS/`. AppKit takes the
    /// main bundle from the executable's path, so a cua-spacesd in
    /// `Contents/MacOS` checks in with LaunchServices *as the app* (same
    /// bundle id and name) when its cursor overlay starts AppKit. Opening
    /// the app then activates that windowless helper instead of launching
    /// the UI. From `Contents/Helpers` it has no bundle of its own and runs
    /// as a nameless background UI element (`cua_host::service::
    /// APP_HELPER_DAEMON`; `scripts/build-app.sh` puts it there).
    public static let daemonInContents = "Helpers/cua-spacesd"

    /// The cua-spacesd of the app whose executable is `executable`
    /// (`<App>.app/Contents/MacOS/<exe>`): `Contents/Helpers/cua-spacesd`.
    public static func daemonPath(executable: URL) -> URL {
        executable.resolvingSymlinksInPath()
            .deletingLastPathComponent() // Contents/MacOS
            .deletingLastPathComponent() // Contents
            .appendingPathComponent(daemonInContents)
    }

    private static func stamped(_ info: [String: Any]?, _ key: String) -> String? {
        guard let value = (info?[key] as? String)?.trimmingCharacters(in: .whitespaces),
              !value.isEmpty else { return nil }
        return value
    }

    public static var label: String { label(info: Bundle.main.infoDictionary) }
    public static var plistName: String { plistName(info: Bundle.main.infoDictionary) }

    static var service: SMAppService { SMAppService.agent(plistName: plistName) }

    public static func status() -> AppLoginItemStatus { MainAppLoginItem.map(service.status) }

    static func name(_ s: AppLoginItemStatus) -> String {
        switch s {
        case .enabled: return "enabled"
        case .notRegistered: return "not-registered"
        case .requiresApproval: return "requires-approval"
        case .notFound: return "not-found"
        }
    }

    /// The command line: prints one JSON object, returns the exit code.
    public static func run(_ args: [String]) -> Int32 {
        var failure: String?
        switch args.first {
        case "register":
            do { try service.register() } catch { failure = error.localizedDescription }
        case "unregister":
            // Not registered is already done.
            if service.status != .notRegistered {
                do { try service.unregister() } catch { failure = error.localizedDescription }
            }
        case "status": break
        default:
            FileHandle.standardError.write(Data("usage: CuaSpacesMac --service-helper register|unregister|status\n".utf8))
            return 2
        }
        var out: [String: String] = ["status": name(status())]
        if let failure { out["error"] = failure }
        if let data = try? JSONSerialization.data(withJSONObject: out, options: [.sortedKeys]) {
            FileHandle.standardOutput.write(data + Data("\n".utf8))
        }
        return failure == nil ? 0 : 1
    }
}
