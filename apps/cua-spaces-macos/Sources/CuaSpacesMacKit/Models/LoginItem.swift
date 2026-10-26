// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSpacesFFI
import Darwin
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

    /// A file's identity (device and inode). An update (Sparkle, or the
    /// app dragged over) puts new files at the same paths, so the same path
    /// with a new identity is a new build.
    public struct FileIdentity: Equatable, Sendable {
        public let device: UInt64
        public let inode: UInt64
        public init(device: UInt64, inode: UInt64) {
            self.device = device
            self.inode = inode
        }
    }

    /// `path`'s identity, or `nil` when it is gone.
    public static func identity(_ path: String) -> FileIdentity? {
        var st = stat()
        guard stat(path, &st) == 0 else { return nil }
        return FileIdentity(device: UInt64(st.st_dev), inode: UInt64(st.st_ino))
    }

    /// What the running service does after one look at its files.
    public enum WatchVerdict: Equatable, Sendable {
        /// Nothing changed.
        case keep
        /// The app's files or the spec are gone (a downgrade to a build
        /// without the service, or the host was removed): stop, exit 0, and
        /// launchd leaves it stopped.
        case stop
        /// The app was updated in place: stop the old cua-spacesd and exit
        /// non-zero, so launchd starts the new build's (KeepAlive
        /// SuccessfulExit=false).
        case restart
    }

    /// One look at the service's files: `started` are the identities of the
    /// files it runs from (its own executable and Contents/Helpers/
    /// cua-spacesd) when it started, `now` the same files now (`nil` for one
    /// that is gone). Gone twice in a row stops (an update's swap is quick
    /// and is not mistaken for a removal); all there with any identity
    /// changed restarts.
    public static func watch(started: [FileIdentity?], now: [FileIdentity?], specPresent: Bool,
                             misses: inout Int) -> WatchVerdict {
        if !specPresent || now.contains(where: { $0 == nil }) {
            misses += 1
            return misses >= 2 ? .stop : .keep
        }
        misses = 0
        return started != now ? .restart : .keep
    }

    /// The exit status of a service that restarts onto an updated app
    /// (EX_TEMPFAIL): non-zero, so launchd starts it again.
    public static let restartExitCode: Int32 = 75

    /// The service started from an older build than the one installed now:
    /// it started before this app's cua-spacesd was put in place (an update
    /// writes new files, so their birth time is when the update landed).
    public static func isStale(serviceStarted: Date, daemonInstalled: Date) -> Bool {
        serviceStarted < daemonInstalled
    }

    /// When `path` came into being (`st_birthtime`), or `nil` when it is gone.
    public static func birthTime(_ path: String) -> Date? {
        var st = stat()
        guard stat(path, &st) == 0 else { return nil }
        let t = st.st_birthtimespec
        return Date(timeIntervalSince1970: TimeInterval(t.tv_sec) + TimeInterval(t.tv_nsec) / 1e9)
    }

    /// When process `pid` started, or `nil` when there is none.
    static func processStart(_ pid: pid_t) -> Date? {
        var info = proc_bsdinfo()
        let size = Int32(MemoryLayout<proc_bsdinfo>.size)
        guard proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &info, size) == size else { return nil }
        return Date(timeIntervalSince1970: TimeInterval(info.pbi_start_tvsec) + TimeInterval(info.pbi_start_tvusec) / 1e6)
    }

    /// The running service's pid, from `launchctl print` (`nil`: not loaded
    /// or not running).
    static func runningPid(target: String) -> pid_t? {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/bin/launchctl")
        p.arguments = ["print", target]
        let out = Pipe()
        p.standardOutput = out
        p.standardError = FileHandle.nullDevice
        guard (try? p.run()) != nil else { return nil }
        let data = out.fileHandleForReading.readDataToEndOfFile()
        p.waitUntilExit()
        return parsePid(String(decoding: data, as: UTF8.self))
    }

    /// The `pid = N` line of `launchctl print`.
    static func parsePid(_ text: String) -> pid_t? {
        for line in text.split(separator: "\n") {
            let t = line.trimmingCharacters(in: .whitespaces)
            if t.hasPrefix("pid = "), let n = Int32(t.dropFirst(6)) { return n }
        }
        return nil
    }

    /// CUA-2457: at launch, the login item still running from before an
    /// update (Sparkle, or the app dragged over) is restarted onto this
    /// build (`launchctl kickstart -k`), so the host service runs the
    /// cua-spacesd that shipped with it. Services from this build watch
    /// their own files and restart themselves; this catches the ones that
    /// started before that watch existed. Off the main thread.
    public static func restartIfStale(executable: URL? = Bundle.main.executableURL) {
        guard let executable else { return }
        let target = "gui/\(getuid())/\(label)"
        DispatchQueue.global(qos: .utility).async {
            guard let pid = runningPid(target: target), let started = processStart(pid),
                  let installed = birthTime(daemonPath(executable: executable).path),
                  isStale(serviceStarted: started, daemonInstalled: installed) else { return }
            NSLog("cua-spaces: the host service (pid %d) is from before this build; restarting it", pid)
            let p = Process()
            p.executableURL = URL(fileURLWithPath: "/bin/launchctl")
            p.arguments = ["kickstart", "-k", target]
            try? p.run()
            p.waitUntilExit()
        }
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
            // Not registered is already done. So is an item that ends up
            // not registered or not found (a bundle moved or replaced since
            // it registered), even when macOS said no on the way.
            if service.status != .notRegistered {
                do { try service.unregister() } catch {
                    if status() != .notRegistered, status() != .notFound { failure = error.localizedDescription }
                }
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
