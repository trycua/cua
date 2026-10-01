// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
import Foundation

// The account, telemetry and coding-agent setup, behind protocols so
// fixtures and tests never touch the account's real session, telemetry
// config or agent configs. Rows and words are the core's.

/// Coding agents on this machine (the SDK's agent onboarding).
public protocol AgentSetupRunning: AnyObject, Sendable {
    /// Every supported agent, as the core reads it.
    func statuses() async -> [AppAgentSetupStatus]
    /// Skills bundled with the SDK.
    func skillsTotal() -> UInt32
    /// Installs the cua skills and/or configures the cua MCP server.
    func setUp(agents: [String], skills: Bool, mcp: Bool) async throws -> [AppAgentSetupOutcomeInput]
    /// Background computer-use: the cua-driver skill and its MCP server
    /// (the same step as `cua agents setup --cua-driver`).
    func setUpCuaDriver(agents: [String]) async throws -> [AppAgentSetupOutcomeInput]
    /// Removes what cua added.
    func remove(agents: [String]) async throws -> [AppAgentSetupOutcomeInput]
}

/// The SDK's `AgentSetup`, registering the app's own `cua` as the MCP
/// server when the bundle carries one.
public final class LiveAgentSetup: AgentSetupRunning, @unchecked Sendable {
    let inner: CuaSDK.AgentSetup
    let server: AgentMcpServer?

    public init(inner: CuaSDK.AgentSetup, cuaBinary: String?) {
        self.inner = inner
        self.server = cuaBinary.map { AgentMcpServer(name: "cua", command: $0, args: ["mcp"], env: [:]) }
    }

    public func statuses() async -> [AppAgentSetupStatus] { inner.detect().map { appAgentSetupStatus(info: $0) } }
    public func skillsTotal() -> UInt32 { UInt32(inner.skills().count) }

    public func setUp(agents: [String], skills: Bool, mcp: Bool) async throws -> [AppAgentSetupOutcomeInput] {
        var out: [AgentSetupOutcome] = []
        if skills { out += try await inner.installSkills(agents: agents, skills: [], force: false) }
        if mcp { out += try await inner.configureMcp(agents: agents, server: server) }
        return out.map { appAgentSetupOutcome(outcome: $0) }
    }

    public func setUpCuaDriver(agents: [String]) async throws -> [AppAgentSetupOutcomeInput] {
        try await inner.setupCuaDriver(agents: agents, command: nil, skills: true, mcp: true)
            .map { appAgentSetupOutcome(outcome: $0) }
    }

    public func remove(agents: [String]) async throws -> [AppAgentSetupOutcomeInput] {
        try await inner.remove(agents: agents, skills: true, mcp: true).map { appAgentSetupOutcome(outcome: $0) }
    }
}

/// In-memory agents for fixtures and tests: nothing on disk is read or written.
public final class FixtureAgentSetup: AgentSetupRunning, @unchecked Sendable {
    public private(set) var current: [AppAgentSetupStatus]
    public private(set) var calls: [String] = []

    public init(statuses: [AppAgentSetupStatus]? = nil) {
        current = statuses ?? [
            AppAgentSetupStatus(id: "claude-code", name: "Claude Code", installed: true, skillsDir: "~/.claude/skills",
                                mcpConfig: "~/.claude.json", cuaConfigured: false, skillsInstalled: [],
                                skillsOutdated: [], error: nil),
            AppAgentSetupStatus(id: "codex", name: "Codex", installed: true, skillsDir: "~/.agents/skills",
                                mcpConfig: "~/.codex/config.toml", cuaConfigured: true,
                                skillsInstalled: ["cua-spaces", "cua-sandbox"], skillsOutdated: [], error: nil),
            AppAgentSetupStatus(id: "cursor", name: "Cursor", installed: false, skillsDir: nil, mcpConfig: nil,
                                cuaConfigured: false, skillsInstalled: [], skillsOutdated: [], error: nil),
        ]
    }

    public func statuses() async -> [AppAgentSetupStatus] { current }
    public func skillsTotal() -> UInt32 { 2 }

    public func setUp(agents: [String], skills: Bool, mcp: Bool) async throws -> [AppAgentSetupOutcomeInput] {
        calls.append("setup:\(agents.joined(separator: ","))")
        var out: [AppAgentSetupOutcomeInput] = []
        for i in current.indices where agents.contains(current[i].id) {
            if skills, current[i].skillsDir != nil {
                current[i].skillsInstalled = ["cua-spaces", "cua-sandbox"]
                out.append(AppAgentSetupOutcomeInput(agents: [current[i].id], target: "skill", item: "cua-spaces",
                                                     change: "created", detail: ""))
            }
            if mcp, current[i].mcpConfig != nil {
                current[i].cuaConfigured = true
                out.append(AppAgentSetupOutcomeInput(agents: [current[i].id], target: "mcp", item: "cua",
                                                     change: "created", detail: ""))
            }
        }
        return out
    }

    public func setUpCuaDriver(agents: [String]) async throws -> [AppAgentSetupOutcomeInput] {
        calls.append("driver:\(agents.joined(separator: ","))")
        var out: [AppAgentSetupOutcomeInput] = []
        for a in current where agents.contains(a.id) {
            if a.skillsDir != nil {
                out.append(AppAgentSetupOutcomeInput(agents: [a.id], target: "skill", item: "cua-driver",
                                                     change: "created", detail: ""))
            }
            if a.mcpConfig != nil {
                out.append(AppAgentSetupOutcomeInput(agents: [a.id], target: "mcp", item: "cua-driver",
                                                     change: "created", detail: ""))
            }
        }
        return out
    }

    public func remove(agents: [String]) async throws -> [AppAgentSetupOutcomeInput] {
        calls.append("remove:\(agents.joined(separator: ","))")
        for i in current.indices where agents.contains(current[i].id) {
            current[i].skillsInstalled = []
            current[i].cuaConfigured = false
        }
        return []
    }
}

/// The usage-telemetry switch, and where the app's usage events go.
public protocol TelemetryRunning: AnyObject, Sendable {
    func status() -> AppTelemetryInput
    func setEnabled(_ on: Bool) throws -> AppTelemetryInput
    /// Records signals the app core derived (`appTelemetry*`): the same ones
    /// the Tauri app sends for the same step.
    func record(_ signals: [AppTelemetrySignal])
}

/// The SDK's telemetry setting (`$CUA_HOME/config.toml`, the same switch as
/// `cua telemetry off`); the environment wins and locks it.
public final class LiveTelemetry: TelemetryRunning, @unchecked Sendable {
    public init() {}

    static func input(_ s: TelemetryStatus) -> AppTelemetryInput {
        let locked = s.sourceKind != "config" && s.sourceKind != "default"
        return AppTelemetryInput(enabled: s.enabled, lockedBy: locked ? s.source : nil)
    }

    public func status() -> AppTelemetryInput { Self.input(telemetryStatus()) }
    public func setEnabled(_ on: Bool) throws -> AppTelemetryInput { Self.input(try telemetrySetEnabled(enabled: on)) }
    /// On the SDK's client: the switch, the first-run notice and the schema
    /// apply (fixed words only).
    public func record(_ signals: [AppTelemetrySignal]) {
        guard !signals.isEmpty else { return }
        _ = appTelemetryRecord(signals: signals)
    }
}

/// An in-memory switch (fixtures, tests).
public final class FixtureTelemetry: TelemetryRunning, @unchecked Sendable {
    public var current = AppTelemetryInput(enabled: true, lockedBy: nil)
    public init() {}
    public func status() -> AppTelemetryInput { current }
    public func setEnabled(_ on: Bool) throws -> AppTelemetryInput {
        current.enabled = on
        return current
    }
    /// What the app recorded, in order (nothing leaves the process).
    public private(set) var recorded: [AppTelemetrySignal] = []
    public func record(_ signals: [AppTelemetrySignal]) {
        if current.enabled { recorded += signals }
    }
}

/// A started sign-in: the code to confirm (device flow), then the identity.
public struct SignInAttempt: Sendable {
    public var userCode: String?
    public var wait: @Sendable () async throws -> String?
}

/// The signed-in account's claims, for the presence name and id.
public struct AccountProfile: Sendable, Equatable {
    public var name: String?
    public var email: String?
    public var username: String?
    public var subject: String?
    public init(name: String? = nil, email: String? = nil, username: String? = nil, subject: String? = nil) {
        self.name = name
        self.email = email
        self.username = username
        self.subject = subject
    }
}

/// The Cua account.
public protocol AccountRunning: AnyObject, Sendable {
    /// The signed-in identity, read without network access.
    func identity() -> String?
    /// The signed-in account's claims, read without network access.
    func profile() -> AccountProfile?
    /// Starts a sign-in (opens the browser).
    func beginSignIn() async throws -> SignInAttempt
    /// Signs out.
    func signOut() async throws
}

extension AccountRunning {
    /// Accounts that only know a display identity: an email or a username.
    public func profile() -> AccountProfile? {
        identity().map { $0.contains("@") ? AccountProfile(email: $0) : AccountProfile(username: $0) }
    }
}

/// The cua SDK's auth (the same session as `cua auth login`).
public final class LiveAccount: AccountRunning, @unchecked Sendable {
    let auth: CuaSDK.Auth

    public init(auth: CuaSDK.Auth) { self.auth = auth }

    static func display(_ i: AuthIdentity?) -> String? {
        i.flatMap { $0.display ?? $0.email ?? $0.username }
    }

    public func identity() -> String? {
        guard let s = try? auth.status(), s.loggedIn else { return nil }
        return Self.display(s.identity)
    }

    public func profile() -> AccountProfile? {
        guard let s = try? auth.status(), s.loggedIn, let i = s.identity else { return nil }
        return AccountProfile(name: i.name, email: i.email, username: i.username, subject: i.subject)
    }

    public func beginSignIn() async throws -> SignInAttempt {
        let attempt = try await auth.beginLogin(flow: nil)
        if let url = URL(string: attempt.url()) {
            await MainActor.run { _ = NSWorkspace.shared.open(url) }
        }
        return SignInAttempt(userCode: attempt.userCode()) {
            Self.display(try await attempt.wait())
        }
    }

    public func signOut() async throws { _ = try await auth.logout() }
}

/// A fixture account (captures, tests): signing in answers at once.
public final class FixtureAccount: AccountRunning, @unchecked Sendable {
    public var current: String?
    public init(identity: String? = nil) { current = identity }
    public func identity() -> String? { current }
    public func beginSignIn() async throws -> SignInAttempt {
        SignInAttempt(userCode: nil) { [weak self] in
            self?.current = "you@example.com"
            return "you@example.com"
        }
    }

    public func signOut() async throws { current = nil }
}

// MARK: - Cua Cloud billing

/// The account's Cua Cloud billing through the cua SDK (`Fleet.billingStatus`):
/// the credit left and the website billing page. The app takes no payment
/// details; the website does.
public protocol BillingRunning: AnyObject, Sendable {
    /// The status as the app core takes it.
    func status() async throws -> AppBillingStatus
}

/// The SDK's Fleet billing status.
public final class LiveBilling: BillingRunning, @unchecked Sendable {
    let cua: Cua
    public init(cua: Cua) { self.cua = cua }

    public func status() async throws -> AppBillingStatus {
        appBillingStatus(status: try await cua.fleet().billingStatus())
    }
}

/// A fixture account's billing (captures, tests): $10 of credit.
public final class FixtureBilling: BillingRunning, @unchecked Sendable {
    public var current: AppBillingStatus

    public init(balanceCents: Int64 = 1000, enabled: Bool = true) {
        current = AppBillingStatus(
            billingEnabled: enabled, card: nil,
            credit: AppBillingCredit(balanceUsdCents: balanceCents),
            billingUrl: "https://run.cua.ai/billing")
    }

    public func status() async throws -> AppBillingStatus { current }
}

/// Opens the website billing page (Settings' Manage billing, a refused
/// cloud Space's Add credit). In debug builds `CUA_SPACES_BROWSER` names
/// another app to open it with (captures use their own window, never the
/// person's browser).
@MainActor
public enum BillingBrowser {
    public static var open: (URL) -> Void = { url in
        if let app = DevHooks.value("CUA_SPACES_BROWSER"), !app.isEmpty {
            NSWorkspace.shared.open([url], withApplicationAt: URL(fileURLWithPath: app),
                                    configuration: NSWorkspace.OpenConfiguration())
        } else {
            NSWorkspace.shared.open(url)
        }
    }
}
