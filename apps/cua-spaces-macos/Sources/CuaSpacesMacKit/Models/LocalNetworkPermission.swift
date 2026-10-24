// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
import Darwin
import Foundation
import Network

// macOS Local Network access before a macOS Space is created on this Mac.
//
// The app's daemon reaches its macOS VMs on Apple's VM network inside this
// Mac, and macOS counts the daemon this app starts as the app: until "Cua
// Spaces" is allowed under System Settings › Privacy & Security › Local
// Network, every connect to a VM fails with "No route to host". So before
// the first macOS Space the app explains why, then makes macOS ask (a
// connect from this process), at a moment the person expects, instead of
// at launch or two minutes into a create that then fails.
//
// The core decides and words every step (`appLocalNetworkStep`); this file
// only probes (a connect, the routing table, whether macOS's prompt is on
// screen) and shows the core's dialogs.

/// The platform probes behind the check.
public protocol LocalNetworkProbing: Sendable {
    /// One connect from this process to the local network: allowed, blocked
    /// by macOS, or unknown (no local network, no clear answer). A connect
    /// while access is undecided makes macOS ask.
    func access() async -> AppLocalNetworkAccess
    /// `netstat -rn -f inet` (empty when it could not be read).
    func routes() async -> String
    /// How many of macOS's permission prompts are on screen.
    func promptsShowing() -> Int
}

/// A create stopped by the check: the core's sentence, and its error
/// variant for telemetry (`local_network_denied`, `local_network_vpn`).
public struct LocalNetworkRefusal: LocalizedError, Equatable {
    public let message: String
    public let variant: String
    public var errorDescription: String? { message }
}

/// Runs the core's Local Network steps before a macOS Space is created on
/// this Mac.
@MainActor
public final class LocalNetworkGate {
    let probe: LocalNetworkProbing
    /// Shows a step's dialog; true when its main button was clicked.
    let present: @MainActor (AppLocalNetworkStep) async -> Bool
    /// Opens a System Settings pane.
    let open: @MainActor (String) -> Void
    let defaults: UserDefaults
    /// How long to wait for macOS's prompt to appear, then to be answered.
    var promptAppears: TimeInterval = 3
    var promptAnswered: TimeInterval = 60
    var poll: Duration = .milliseconds(500)

    static let askedKey = "localNetworkExplained"

    public init(probe: LocalNetworkProbing,
                present: @escaping @MainActor (AppLocalNetworkStep) async -> Bool = LocalNetworkGate.alert,
                open: @escaping @MainActor (String) -> Void = LocalNetworkGate.openSettings,
                defaults: UserDefaults = .standard) {
        self.probe = probe
        self.present = present
        self.open = open
        self.defaults = defaults
    }

    /// The app already explained and made macOS ask.
    var asked: Bool {
        get { defaults.bool(forKey: Self.askedKey) }
        set { defaults.set(newValue, forKey: Self.askedKey) }
    }

    /// Returns when the create may go ahead; throws `LocalNetworkRefusal`
    /// (the create fails at once with it) or `CuaError.Cancelled` (Cancel
    /// on the explanation).
    public func check() async throws {
        let routes = await probe.routes()
        // Not explained yet: no connect before the explanation (one would
        // make macOS ask first).
        let first: AppLocalNetworkAccess = asked ? await probe.access() : .notChecked
        var step = appLocalNetworkStep(input: AppLocalNetworkInput(access: first, asked: asked, routes: routes))
        if step.kind == .explain {
            guard await present(step) else {
                throw CuaError.Cancelled(message: "create was cancelled by user")
            }
            asked = true
            let access = await waitForAnswer()
            step = appLocalNetworkStep(input: AppLocalNetworkInput(access: access, asked: true, routes: routes))
        }
        guard step.kind == .fail else { return }
        let shown = step
        // The row fails now; the dialog with the fix shows meanwhile.
        Task { @MainActor [present, open] in
            if await present(shown), let url = shown.settingsUrl { open(url) }
        }
        throw LocalNetworkRefusal(message: step.message, variant: step.errorVariant)
    }

    /// Makes macOS ask (a connect while access is undecided) and waits for
    /// the answer: allowed as soon as a connect goes out; otherwise what a
    /// connect finds once the prompt is gone (or never came).
    func waitForAnswer() async -> AppLocalNetworkAccess {
        let before = probe.promptsShowing()
        let start = Date()
        var appeared = false
        while true {
            let access = await probe.access()
            if access == .allowed { return access }
            let elapsed = Date().timeIntervalSince(start)
            if probe.promptsShowing() > before {
                appeared = true
            } else if appeared || elapsed >= promptAppears {
                return access
            }
            if elapsed >= promptAnswered { return access }
            try? await Task.sleep(for: poll)
        }
    }

    /// The core's dialog as a native alert: a sheet on the window in front
    /// (the app keeps running meanwhile; New UI's page keeps its answers),
    /// else a window of its own.
    public static func alert(_ step: AppLocalNetworkStep) async -> Bool {
        let alert = NSAlert()
        alert.messageText = step.title
        alert.informativeText = step.message
        alert.alertStyle = step.kind == .fail ? .warning : .informational
        alert.addButton(withTitle: step.primary)
        if !step.secondary.isEmpty { alert.addButton(withTitle: step.secondary) }
        NSApp.activate()
        let window = NSApp.keyWindow ?? NSApp.mainWindow
            ?? NSApp.windows.first { $0.isVisible && $0.canBecomeMain && $0.attachedSheet == nil }
        guard let window, window.attachedSheet == nil else {
            return alert.runModal() == .alertFirstButtonReturn
        }
        return await withCheckedContinuation { done in
            alert.beginSheetModal(for: window) { done.resume(returning: $0 == .alertFirstButtonReturn) }
        }
    }

    public static func openSettings(_ url: String) {
        guard url.hasPrefix("x-apple.systempreferences:"), let u = URL(string: url) else { return }
        NSWorkspace.shared.open(u)
    }
}

/// The probes on this Mac.
public struct LiveLocalNetworkProbe: LocalNetworkProbing {
    /// How long one connect may take before it says nothing.
    let timeout: TimeInterval

    public init(timeout: TimeInterval = 2.5) {
        self.timeout = timeout
    }

    public func access() async -> AppLocalNetworkAccess {
        guard let host = LiveLocalNetworkPermission.firstSubnetHost() else { return .unknown }
        let queue = DispatchQueue(label: "com.trycua.cua-spaces.local-network.check")
        return await withCheckedContinuation { (done: CheckedContinuation<AppLocalNetworkAccess, Never>) in
            // Port 9 (discard): nothing has to listen; a refusal comes back
            // over the network, so it went out.
            let connection = NWConnection(host: NWEndpoint.Host(host), port: 9, using: .tcp)
            var finished = false
            func finish(_ result: AppLocalNetworkAccess) {
                guard !finished else { return }
                finished = true
                connection.cancel()
                done.resume(returning: result)
            }
            connection.stateUpdateHandler = { state in
                switch state {
                case .ready:
                    finish(.allowed)
                case .waiting(let error), .failed(let error):
                    if connection.currentPath?.unsatisfiedReason == .localNetworkDenied {
                        finish(.blocked)
                    } else if case .posix(let code) = error, code == .ECONNREFUSED || code == .ECONNRESET {
                        finish(.allowed)
                    } else if case .posix(let code) = error, code == .EHOSTUNREACH {
                        finish(.blocked)
                    } else {
                        finish(.unknown)
                    }
                default:
                    break
                }
            }
            connection.start(queue: queue)
            queue.asyncAfter(deadline: .now() + timeout) { finish(.unknown) }
        }
    }

    public func routes() async -> String {
        await Task.detached {
            let process = Process()
            process.executableURL = URL(fileURLWithPath: "/usr/sbin/netstat")
            process.arguments = ["-rn", "-f", "inet"]
            let out = Pipe()
            process.standardOutput = out
            process.standardError = FileHandle.nullDevice
            guard (try? process.run()) != nil else { return "" }
            let data = out.fileHandleForReading.readDataToEndOfFile()
            process.waitUntilExit()
            return String(decoding: data, as: UTF8.self)
        }.value
    }

    /// macOS's permission prompts are UserNotificationCenter's windows.
    public func promptsShowing() -> Int {
        guard let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] else {
            return 0
        }
        return list.filter { ($0[kCGWindowOwnerName as String] as? String) == "UserNotificationCenter" }.count
    }
}

/// Where the probes aim: this Mac's first local network.
public enum LiveLocalNetworkPermission {
    /// The first active local IPv4 interface's first host (usually the
    /// router); not loopback, link-local, a VPN's tunnel or the VMs' own
    /// bridge.
    static func firstSubnetHost() -> String? {
        var list: UnsafeMutablePointer<ifaddrs>?
        guard getifaddrs(&list) == 0, let first = list else { return nil }
        defer { freeifaddrs(list) }
        for cursor in sequence(first: first, next: { $0.pointee.ifa_next }) {
            let ifa = cursor.pointee
            let flags = Int32(ifa.ifa_flags)
            let name = String(cString: ifa.ifa_name)
            guard flags & IFF_UP != 0, flags & IFF_RUNNING != 0, flags & IFF_LOOPBACK == 0,
                  !isTunnelOrBridge(name),
                  let addr = ifa.ifa_addr, addr.pointee.sa_family == sa_family_t(AF_INET),
                  let mask = ifa.ifa_netmask else { continue }
            let ip = addr.withMemoryRebound(to: sockaddr_in.self, capacity: 1) { UInt32(bigEndian: $0.pointee.sin_addr.s_addr) }
            let m = mask.withMemoryRebound(to: sockaddr_in.self, capacity: 1) { UInt32(bigEndian: $0.pointee.sin_addr.s_addr) }
            if let host = subnetHost(ip: ip, mask: m) { return host }
        }
        return nil
    }

    static func isTunnelOrBridge(_ name: String) -> Bool {
        ["bridge", "utun", "ipsec", "ppp", "tun", "tap", "wg"].contains { name.hasPrefix($0) }
    }

    /// The subnet's first host (`192.168.1.23/24` → `192.168.1.1`); nil for
    /// link-local or a subnet with no room for another host.
    static func subnetHost(ip: UInt32, mask: UInt32) -> String? {
        guard ip >> 16 != 0xA9FE, mask != 0, ~mask >= 2 else { return nil }
        var target = (ip & mask) + 1
        if target == ip { target += 1 }
        return [24, 16, 8, 0].map { String((target >> UInt32($0)) & 0xFF) }.joined(separator: ".")
    }
}
