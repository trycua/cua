// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpacesFFI
import Darwin
import Foundation
import ServiceManagement

/// Cua Driver's own login item (CUA-3638): this app's executable
/// (`CuaSpacesMac --driver-helper run`, so the driver's permissions are the
/// app's) run by `SMAppService.agent` from
/// `Contents/Library/LaunchAgents/<label>.plist`
/// (com.trycua.spaces.driver.plist for the released app). It runs
/// `DriverSupervisor`, which keeps `Contents/Helpers/cua-driver serve
/// --embedded` up for the coding agents on this Mac.
///
/// It is not the host service (`HelperService`): that one runs only while
/// this Mac is set up in Cua Spaces and signed in to the account it was set
/// up with, and is uninstalled while paused. Cua Driver for this Mac's own
/// agents needs none of that, so it has its own item, which the app
/// registers at launch and which only the person's "Let agents use apps on
/// this Mac" switch turns off.
public enum DriverService {
    /// The driver item's label: the host service's (`HelperService.label`)
    /// with `.driver` for `.helper`, so each channel has its own
    /// (com.trycua.spaces.driver, com.trycua.spaces.local.driver).
    public static func label(info: [String: Any]?) -> String {
        let helper = HelperService.label(info: info)
        let base = helper.hasSuffix(".helper") ? String(helper.dropLast(".helper".count)) : helper
        return base + ".driver"
    }

    /// The plist in Contents/Library/LaunchAgents: `<label>.plist`.
    public static func plistName(info: [String: Any]?) -> String {
        label(info: info) + ".plist"
    }

    public static var label: String { label(info: Bundle.main.infoDictionary) }

    static var service: SMAppService { SMAppService.agent(plistName: plistName(info: Bundle.main.infoDictionary)) }

    /// Whether `bundle` carries the driver item (every build from 0.21.1).
    public static func isBundled(_ bundle: Bundle = .main) -> Bool {
        let plist = bundle.bundleURL
            .appendingPathComponent("Contents/Library/LaunchAgents")
            .appendingPathComponent(plistName(info: bundle.infoDictionary))
        return FileManager.default.fileExists(atPath: plist.path)
    }

    public static func status() -> AppLoginItemStatus { MainAppLoginItem.map(service.status) }

    /// What the page says when macOS holds the item back.
    public static let needsApproval =
        "Cua Spaces isn't allowed to run in the background, so Cua Driver can't start. Turn on Cua Spaces in System Settings > General > Login Items & Extensions, then choose Repair."

    /// Registers the item so launchd has its job, and starts it
    /// (`launchctl kickstart`, `-k` when `restart`). An item macOS still
    /// counts as enabled but launchd has unloaded (`launchctl bootout`, a
    /// crash while the app was replaced) is unregistered and registered
    /// again (`LaunchAgentRegistration`), so launch and Repair heal it
    /// without a logout. Blocks: call off the main thread. Returns why it
    /// couldn't, in plain words, or nil.
    @discardableResult
    public static func start(restart: Bool) -> String? {
        do {
            let step = try register()
            if step == .reregister { NSLog("cua-spaces: %@ was enabled but not loaded; registered it again", label) }
        } catch {
            if status() == .requiresApproval { return needsApproval }
            return "Couldn't start Cua Driver in the background: \(error.localizedDescription)"
        }
        if status() == .requiresApproval { return needsApproval }
        if !kickstart(restart: restart) {
            return "Couldn't start Cua Driver in the background: launchd didn't start \(label)"
        }
        return nil
    }

    /// Registers the item, or registers it again when launchd lost its job.
    @discardableResult
    static func register() throws -> LaunchAgentRegistration.Step {
        try LaunchAgentRegistration.ensureLoaded(LaunchAgentItem(plistName: plistName(info: Bundle.main.infoDictionary)),
                                                 isLoaded: { LaunchAgentRegistration.isLoaded(label: label) })
    }

    /// `launchctl kickstart [-k] gui/<uid>/<label>`. False when launchctl
    /// failed (the item isn't loaded).
    @discardableResult
    public static func kickstart(restart: Bool) -> Bool {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/bin/launchctl")
        p.arguments = ["kickstart"] + (restart ? ["-k"] : []) + ["gui/\(getuid())/\(label)"]
        p.standardOutput = FileHandle.nullDevice
        p.standardError = FileHandle.nullDevice
        guard (try? p.run()) != nil else { return false }
        p.waitUntilExit()
        return p.terminationStatus == 0
    }

    /// `CuaSpacesMac --driver-helper register|unregister|status`: prints one
    /// JSON object (`status`, and `error`), returns the exit code.
    public static func run(_ args: [String]) -> Int32 {
        var failure: String?
        switch args.first {
        case "register":
            do { try register() } catch { failure = error.localizedDescription }
        case "unregister":
            if service.status != .notRegistered {
                do { try service.unregister() } catch {
                    if status() != .notRegistered, status() != .notFound { failure = error.localizedDescription }
                }
            }
        case "status": break
        default:
            FileHandle.standardError.write(Data("usage: CuaSpacesMac --driver-helper run|register|unregister|status\n".utf8))
            return 2
        }
        var out: [String: String] = ["status": HelperService.name(status())]
        if let failure { out["error"] = failure }
        if let data = try? JSONSerialization.data(withJSONObject: out, options: [.sortedKeys]) {
            FileHandle.standardOutput.write(data + Data("\n".utf8))
        }
        return failure == nil ? 0 : 1
    }
}
