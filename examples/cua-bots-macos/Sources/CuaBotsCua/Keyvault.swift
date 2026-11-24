// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Cua
import CuaBotsCore
import CuaSpacesFFI
import Foundation

/// "Log me into this site", on the Cua Keyvault. The bot asks with
/// `[[login: site]]`; the user approves in the app; the daemon delivers the
/// saved session for that site into the bot's Space. The bot's model never
/// sees a password, and neither does this app: it only names the item.
@MainActor
public final class KeyvaultBridge: ObservableObject, SignInBroker {
    public let client: KeyvaultClient
    public weak var engine: CuaEngine?
    @Published public private(set) var overview: KeyvaultOverview?

    public init(cuaHome: String? = nil) {
        client = KeyvaultClient(cuaHome: cuaHome)
    }

    public func refresh() async {
        overview = await client.overview()
    }

    /// `ready`, or why not (`not_running`, `no_vault`, `locked`,
    /// `not_first_party`, ...), in words.
    public var availability: String { overview?.availability ?? "unknown" }

    public var isReady: Bool { overview?.availability == "ready" }

    public var statusLine: String {
        guard let o = overview else { return "Checking the Keyvault" }
        if o.availability == "ready" { return "\(o.items.count) saved sign-ins" }
        let text = o.message ?? Self.describe(o.availability)
        return text.hasSuffix(".") ? String(text.dropLast()) : text
    }

    static func describe(_ availability: String) -> String {
        switch availability {
        case "not_running": "Start the Cua daemon to use saved sign-ins"
        case "no_vault": "Set up the Keyvault in Cua to save sign-ins"
        case "locked": "Unlock the Keyvault in Cua"
        case "not_first_party": "Saved sign-ins are shown only in Cua-signed apps"
        case "unsupported": "The Keyvault isn't available on this Mac"
        default: "The Keyvault isn't available"
        }
    }

    /// Saved sign-ins for a site (items whose site matches, subdomains included).
    public func items(for site: String) -> [KvItem] {
        let host = Self.host(site)
        return (overview?.items ?? []).filter { item in
            guard let s = item.site.map(Self.host) else { return false }
            return s == host || host.hasSuffix("." + s) || s.hasSuffix("." + host)
        }
    }

    static func host(_ s: String) -> String {
        let t = s.lowercased().trimmingCharacters(in: .whitespaces)
        let noScheme = t.components(separatedBy: "://").last ?? t
        return String(noScheme.split(separator: "/").first ?? Substring(noScheme))
            .replacingOccurrences(of: "www.", with: "")
    }

    // MARK: - SignInBroker

    public func signIn(bot: Bot, site: String) async throws -> String {
        await refresh()
        guard isReady else { throw SignInFailure.unavailable(statusLine) }
        guard let item = items(for: site).first else { throw SignInFailure.noSavedSignIn(site) }
        guard let engine else { throw SignInFailure.unavailable("No engine") }
        let space = try await engine.space(bot)
        // Teleport the item's app session into the Space. The daemon asks for
        // Touch ID; the approver below only selects what the user picked.
        // #region docs:sw-teleport
        let receipt = try await space.teleport(app: item.providerId, scope: nil,
                                               approver: SelectItems(label: item.label))
        // #endregion docs:sw-teleport
        return "saved sign-in \(item.account ?? item.label), \(receipt.method)"
    }

    /// Approve or deny a request an agent made itself (`teleport_app`).
    public func decide(requestID: String, approve: Bool) async throws {
        _ = try await client.execute(command: approve ? .approve(requestId: requestID, items: nil)
                                                      : .deny(requestId: requestID))
        await refresh()
    }

    public enum SignInFailure: LocalizedError {
        case unavailable(String)
        case noSavedSignIn(String)
        public var errorDescription: String? {
            switch self {
            case .unavailable(let why): why
            case .noSavedSignIn(let site): "There's no saved sign-in for \(site). Take over to sign in yourself."
            }
        }
    }
}

/// Keeps every manifest item the user already chose by approving the card.
final class SelectItems: TeleportApprover, @unchecked Sendable {
    let label: String
    init(label: String) { self.label = label }
    func approve(manifest: TeleportManifest) -> TeleportDecision? {
        TeleportDecision(include: nil, acknowledgeSensitive: true)
    }
}

/// Access to this Mac, per bot, on `cua host`: the Mac becomes a `relay:`
/// Space the bot can be given; revoking stops sharing. The allowlist is per
/// account today, so every bot the account runs sees the same switch.
@MainActor
public final class HostAccessBridge: ObservableObject {
    public let host: CuaSDK.Host
    @Published public private(set) var status: HostStatus?
    @Published public private(set) var error: String?

    public init(cuaHome: String? = nil) {
        host = CuaSDK.Host(cuaHome: cuaHome)
    }

    public func refresh() async {
        do { status = try await host.status(); error = nil } catch { self.error = error.localizedDescription }
    }

    /// This Mac's name as shown in the app: `CUA_BOTS_DEVICE_NAME`, else the
    /// name the relay knows it by, else the computer name.
    public var machineName: String {
        if let v = ProcessInfo.processInfo.environment["CUA_BOTS_DEVICE_NAME"], !v.isEmpty { return v }
        return status?.name ?? HostAccessBridge.localName
    }

    public var isConfigured: Bool { status?.configured == true }
    public var isSharing: Bool { status?.sharing == true && status?.serviceRunning == true }

    /// Allow: share this Mac (already set up with `cua host setup`).
    public func allow() async throws {
        guard isConfigured else { throw HostError.notSetUp }
        status = try await host.startSharing()
    }

    /// Revoke: stop sharing. Works when the relay is down.
    public func revoke() async throws {
        guard isConfigured else { return }
        status = try await host.stopSharing()
    }

    public enum HostError: LocalizedError {
        case notSetUp
        public var errorDescription: String? {
            "Set up this Mac for unattended access first: run `cua host setup` (or Cua > Host) once."
        }
    }
}

extension HostAccessBridge {
    nonisolated static var localName: String {
        ProcessInfo.processInfo.hostName.replacingOccurrences(of: ".local", with: "")
    }
}
