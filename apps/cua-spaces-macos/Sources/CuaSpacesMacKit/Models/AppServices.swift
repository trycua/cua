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
    /// Starts the agent's own sign-in on this Mac (`codex login` opens the
    /// browser); returns once it runs. The sign-in stays the agent's own.
    func signIn(agent: String) async throws
    /// Whether each agent is really connected (CONTRACT §4), as the JSON
    /// `cua connect status --json` prints. `refresh` skips the SDK's
    /// few-second cache (window focus).
    func connectionStatusJson(refresh: Bool) async -> String
    /// Connect and Reconnect: Cua's tools and, with `signIn`, the agent's
    /// sign-in copied to Keyvault on This Mac. JSON: one result per agent.
    func connectJson(agents: [String], signIn: Bool, skills: Bool, mcp: Bool) async throws -> String
    /// Disconnect: both undone. JSON `{outcomes, removed_sign_ins}`.
    func disconnectJson(agents: [String], signIn: Bool) async throws -> String
    /// Disconnect of some parts only (an app's page's switches).
    func disconnectPartsJson(agents: [String], skills: Bool, mcp: Bool, signIn: Bool) async throws -> String
    /// An agent's device-code sign-in made for Cua (Codex): JSON `{id, url, code}`.
    func deviceSignInStartJson(agent: String) async throws -> String
    /// Waits for it to be approved and saves it in Keyvault: JSON as Connect's `sign_in`.
    func deviceSignInWaitJson(id: String) async throws -> String
}

/// The SDK's `AgentSetup`, registering the app's own `cua` as the MCP
/// server when the bundle carries one. The server has no name here: the
/// SDK names it for the app's channel (`cua`, `cua-local` for Cua Spaces
/// Local), so the Local app never writes over the released app's entry, and
/// it installs the channel's skill copies (`cua-spaces-local`).
public final class LiveAgentSetup: AgentSetupRunning, @unchecked Sendable {
    let inner: CuaSDK.AgentSetup
    let server: AgentMcpServer?

    public init(inner: CuaSDK.AgentSetup, cuaBinary: String?) {
        self.inner = inner
        self.server = cuaBinary.map { AgentMcpServer(name: "", command: $0, args: ["mcp"], env: [:]) }
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

    public func signIn(agent: String) async throws { try inner.signIn(agent: agent) }

    public func connectionStatusJson(refresh: Bool) async -> String {
        let inner = inner, server = server
        return await Task.detached { inner.connectionStatusJson(expected: server, refresh: refresh) }.value
    }

    public func connectJson(agents: [String], signIn: Bool, skills: Bool, mcp: Bool) async throws -> String {
        try await inner.connectJson(agents: agents, server: server, skills: skills, mcp: mcp, signIn: signIn)
    }

    public func disconnectJson(agents: [String], signIn: Bool) async throws -> String {
        try await inner.disconnectJson(agents: agents, signIn: signIn)
    }

    public func disconnectPartsJson(agents: [String], skills: Bool, mcp: Bool, signIn: Bool) async throws -> String {
        try await inner.disconnectPartsJson(agents: agents, skills: skills, mcp: mcp, signIn: signIn)
    }

    public func deviceSignInStartJson(agent: String) async throws -> String {
        try await inner.deviceSignInStartJson(agent: agent)
    }

    public func deviceSignInWaitJson(id: String) async throws -> String {
        try await inner.deviceSignInWaitJson(id: id)
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
                                skillsOutdated: [], error: nil, signIn: nil, signedIn: false),
            AppAgentSetupStatus(id: "codex", name: "Codex", installed: true, skillsDir: "~/.agents/skills",
                                mcpConfig: "~/.codex/config.toml", cuaConfigured: true,
                                skillsInstalled: ["cua-spaces", "cua-sandbox"], skillsOutdated: [], error: nil,
                                signIn: "ChatGPT", signedIn: false),
            AppAgentSetupStatus(id: "cursor", name: "Cursor", installed: false, skillsDir: nil, mcpConfig: nil,
                                cuaConfigured: false, skillsInstalled: [], skillsOutdated: [], error: nil,
                                signIn: nil, signedIn: false),
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

    /// The sign-in finishes at once (no browser in a fixture).
    public func signIn(agent: String) async throws {
        calls.append("sign-in:\(agent)")
        guard let i = current.firstIndex(where: { $0.id == agent }), current[i].signIn != nil else {
            throw NSError(domain: "FixtureAgentSetup", code: 1,
                          userInfo: [NSLocalizedDescriptionKey: "\(agent) has no sign-in cua can start"])
        }
        current[i].signedIn = true
    }

    /// The connection rows the fixture's agents give (the same rules as
    /// `cua_agent_setup::connection`).
    public func connectionStatusJson(refresh: Bool) async -> String {
        let rows: [[String: Any]] = current.map { a in
            let state = !a.installed ? (a.cuaConfigured ? "attention" : "not_installed")
                : !a.cuaConfigured ? "not_connected"
                : (a.signIn != nil && !a.signedIn) ? "attention" : "connected"
            var problem: Any = NSNull()
            if state == "attention" {
                problem = a.installed
                    ? ["line": "Connected. No \(a.signIn ?? "") sign-in on This Mac yet.", "action": "sign_in"]
                    : ["line": "\(a.name) isn't on This Mac anymore. Disconnect takes Cua out of its settings.", "action": "disconnect"]
            }
            return ["id": a.id, "name": a.name, "installed": a.installed, "version": NSNull(), "previous_version": NSNull(),
                    "signed_in": a.signIn == nil ? "unknown" : (a.signedIn ? "yes" : "no"), "sign_in": a.signIn.map { "\($0) sign-in" } ?? NSNull(),
                    "mcp": a.mcpConfig == nil ? "unsupported" : (a.cuaConfigured ? "ok" : "missing"), "mcp_config": a.mcpConfig ?? NSNull(),
                    "signin_imported": false, "signin_import": a.signIn == nil ? "unsupported" : "missing",
                    "state": state, "problem": problem, "checked_at_ms": 0]
        }
        let data = (try? JSONSerialization.data(withJSONObject: ["agents": rows, "checked_at_ms": 0])) ?? Data()
        return String(decoding: data, as: UTF8.self)
    }

    public func connectJson(agents: [String], signIn: Bool, skills: Bool, mcp: Bool) async throws -> String {
        _ = try await setUp(agents: agents, skills: skills, mcp: mcp)
        calls.append("connect:\(agents.joined(separator: ",")):\(signIn)")
        let rows = agents.map { ["agent": $0, "tools_ok": true, "tools_line": NSNull(), "sign_in": NSNull()] as [String: Any] }
        return String(decoding: try JSONSerialization.data(withJSONObject: rows), as: UTF8.self)
    }

    public func disconnectJson(agents: [String], signIn: Bool) async throws -> String {
        _ = try await remove(agents: agents)
        return #"{"outcomes":[],"removed_sign_ins":[]}"#
    }

    public func disconnectPartsJson(agents: [String], skills: Bool, mcp: Bool, signIn: Bool) async throws -> String {
        calls.append("disconnect-parts:\(agents.joined(separator: ",")):\(skills):\(mcp)")
        return #"{"outcomes":[],"removed_sign_ins":[]}"#
    }

    /// Codex's device code, approved at once (no browser in fixtures).
    public func deviceSignInStartJson(agent: String) async throws -> String {
        calls.append("device-start:\(agent)")
        return #"{"id":"\#(agent)-1","url":"https://auth.openai.com/codex/device","code":"ABCD-12345"}"#
    }

    public func deviceSignInWaitJson(id: String) async throws -> String {
        calls.append("device-wait:\(id)")
        return #"{"imported":{"source":"codex","env":"CODEX_AUTH_JSON","label":"ChatGPT sign-in for Codex","expires_ms":null,"added_ms":1},"refused":null,"line":null}"#
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

/// The one-time notice that usage data is now linked to the signed-in
/// account (CUA-1881), for machines that saw the older "anonymous" wording:
/// the SDK's words (`telemetryLinkNotice`) and whether it is due now.
public struct LinkNoticeCopy: Sendable, Equatable {
    public var due: Bool
    public var title: String
    public var body: String
    /// "Got it": keeps usage data on.
    public var okLabel: String
    /// "Turn off usage data".
    public var turnOffLabel: String
    /// The link to what is collected.
    public var detailsLabel: String
    public var docsUrl: String

    public init(due: Bool, title: String, body: String, okLabel: String, turnOffLabel: String,
                detailsLabel: String, docsUrl: String) {
        self.due = due
        self.title = title
        self.body = body
        self.okLabel = okLabel
        self.turnOffLabel = turnOffLabel
        self.detailsLabel = detailsLabel
        self.docsUrl = docsUrl
    }
}

/// The usage-telemetry switch, and where the app's usage events go.
public protocol TelemetryRunning: AnyObject, Sendable {
    func status() -> AppTelemetryInput
    func setEnabled(_ on: Bool) throws -> AppTelemetryInput
    /// Records signals the app core derived (`appTelemetry*`): the same ones
    /// the Tauri app sends for the same step.
    func record(_ signals: [AppTelemetrySignal])
    /// The first run's usage notice was on screen and the person moved on
    /// (left Welcome, finished the first run, or used the switch beside the
    /// note): from here usage events may be sent. Keeps the switch as it is.
    func acknowledgeNotice()

    /// The one-time linking notice: its words and whether it is due now.
    func linkNotice() -> LinkNoticeCopy
    /// The person closed it (either button): linking starts at the next
    /// launch, never in this process.
    func acknowledgeLinkNotice()
}

/// The SDK's telemetry setting (`$CUA_HOME/config.toml`, the same switch as
/// `cua telemetry off`); the environment wins and locks it.
public final class LiveTelemetry: TelemetryRunning, @unchecked Sendable {
    public init() {}

    static func input(_ s: TelemetryStatus) -> AppTelemetryInput {
        let locked = s.sourceKind != "config" && s.sourceKind != "default"
        return AppTelemetryInput(enabled: s.enabled, lockedBy: locked ? s.source : nil, noticeShown: s.noticeShown)
    }

    public func status() -> AppTelemetryInput { Self.input(telemetryStatus()) }
    public func setEnabled(_ on: Bool) throws -> AppTelemetryInput { Self.input(try telemetrySetEnabled(enabled: on)) }
    /// On the SDK's client: the switch, the first-run notice and the schema
    /// apply (fixed words only).
    public func record(_ signals: [AppTelemetrySignal]) {
        guard !signals.isEmpty else { return }
        _ = appTelemetryRecord(signals: signals)
    }
    /// Writes the notice marker (and records the `app_launched` that waited
    /// for it) once; the switch stays as it is.
    public func acknowledgeNotice() {
        let s = telemetryStatus()
        guard !s.noticeShown else { return }
        do { try appTelemetryWelcomeLeft(on: s.enabled) } catch {
            NSLog("Cua Spaces: the usage notice was not recorded: %@", LiveSpacesBackend.words(error))
        }
    }


    public func linkNotice() -> LinkNoticeCopy {
        let n = telemetryLinkNotice()
        return LinkNoticeCopy(due: telemetryLinkNoticeDue(), title: n.title, body: n.body, okLabel: n.okLabel,
                              turnOffLabel: n.turnOffLabel, detailsLabel: n.detailsLabel, docsUrl: n.docsUrl)
    }

    public func acknowledgeLinkNotice() { telemetryAcknowledgeLinkNotice() }
}

/// An in-memory switch (fixtures, tests).
public final class FixtureTelemetry: TelemetryRunning, @unchecked Sendable {
    public var current = AppTelemetryInput(enabled: true, lockedBy: nil, noticeShown: false)
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
    /// How many times the notice was acknowledged.
    public private(set) var acknowledgements = 0
    public func acknowledgeNotice() {
        acknowledgements += 1
        current.noticeShown = true
    }

    /// Whether the one-time linking notice is due (a machine that saw the older wording).
    public var linkNoticeDue = false
    /// How many times the notice was closed.
    public private(set) var linkNoticeAcknowledged = 0
    public func linkNotice() -> LinkNoticeCopy {
        LinkNoticeCopy(due: linkNoticeDue, title: "We've updated how usage data works",
                       body: "Cua collects usage data, like which features you use and when something goes wrong.",
                       okLabel: "Got it", turnOffLabel: "Turn off usage data", detailsLabel: "What is collected",
                       docsUrl: "https://cua.ai/docs/cua-sdk/concepts/telemetry")
    }
    public func acknowledgeLinkNotice() {
        linkNoticeAcknowledged += 1
        linkNoticeDue = false
    }
}

/// A started sign-in: the code to confirm (device flow), then the identity.
public struct SignInAttempt: Sendable {
    public var userCode: String?
    /// The page the sign-in finishes on (to open it again).
    public var url: URL?
    /// A browser opened on this Mac (false: the page is to be opened by
    /// hand, here or on another device).
    public var opened: Bool
    public var wait: @Sendable () async throws -> String?

    public init(userCode: String?, url: URL? = nil, opened: Bool = true,
                wait: @escaping @Sendable () async throws -> String?) {
        self.userCode = userCode
        self.url = url
        self.opened = opened
        self.wait = wait
    }
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
    /// Starts a sign-in with a code to enter on any device (`code`), for when
    /// the browser cannot get back to the app. Accounts without a code flow
    /// start the usual sign-in.
    func beginSignIn(code: Bool) async throws -> SignInAttempt
    /// Signs out.
    func signOut() async throws
}

extension AccountRunning {
    public func beginSignIn(code: Bool) async throws -> SignInAttempt { try await beginSignIn() }

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

    public func beginSignIn() async throws -> SignInAttempt { try await beginSignIn(code: false) }

    public func beginSignIn(code: Bool) async throws -> SignInAttempt {
        // The browser's sign-in page says where to go back to: the SDK reads
        // CUA_AUTH_RETURN_TO, which `TargetEnvironment.pin()` sets once at launch
        // (changing the environment from here races other threads reading it).
        var attempt = try await auth.beginLogin(flow: code ? "device" : nil)
        var opened = await Self.open(attempt.url())
        // No browser opened: the loopback redirect would never come back,
        // so use a code that can be confirmed on any device instead.
        if !opened, attempt.method() == .browser {
            attempt = try await auth.beginLogin(flow: "device")
            opened = await Self.open(attempt.url())
        }
        let started = attempt
        return SignInAttempt(userCode: started.userCode(), url: URL(string: started.url()), opened: opened) {
            Self.display(try await started.wait())
        }
    }

    /// Opens `link` in the default browser; false when nothing opened it.
    static func open(_ link: String) async -> Bool {
        guard let url = URL(string: link), !link.isEmpty else { return false }
        return await MainActor.run { NSWorkspace.shared.open(url) }
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

// MARK: - Teams

/// The account's Teams from the Spaces API (api.cua.ai, api.staging.cua.ai
/// under `CUA_ENV=staging`) through the app core: its organizations with
/// their members and pending invites. The app administers nothing; invites,
/// roles and billing are the account app's (app.cua.ai).
public protocol TeamsReading: AnyObject, Sendable {
    func teams() async throws -> AppTeamsInput
    /// People removed from the account's shared computers because sharing is
    /// for teams now, until acknowledged (none from a server that predates it).
    func shareRemovals() async throws -> [AppShareRemovalInput]
    /// The owner has seen the removal note.
    func ackShareRemovals() async throws
}

public extension TeamsReading {
    func shareRemovals() async throws -> [AppShareRemovalInput] { [] }
    func ackShareRemovals() async throws {}
}

/// Reads Teams with the account's access token (`force`: refresh it).
public final class LiveTeams: TeamsReading, @unchecked Sendable {
    let token: @Sendable (Bool) async throws -> String?
    public init(token: @escaping @Sendable (Bool) async throws -> String?) { self.token = token }

    public func teams() async throws -> AppTeamsInput {
        guard let token = try await token(false) else {
            throw CuaError.Unauthenticated(message: "Sign in to see your teams")
        }
        return try await appTeams(accountToken: token)
    }

    public func shareRemovals() async throws -> [AppShareRemovalInput] {
        guard let token = try await token(false) else { return [] }
        return try await appShareRemovals(accountToken: token)
    }

    public func ackShareRemovals() async throws {
        guard let token = try await token(false) else { return }
        try await appAckShareRemovals(accountToken: token)
    }
}

/// A fixture account's team (captures, tests).
public final class FixtureTeams: TeamsReading, @unchecked Sendable {
    public var current: AppTeamsInput
    private let lock = NSLock()
    private var pending: [AppShareRemovalInput]
    /// Who the fixture's owner has not been told was removed yet. Read by a
    /// test while the acknowledgement clears it on another task.
    public var removals: [AppShareRemovalInput] {
        get { lock.withLock { pending } }
        set { lock.withLock { pending = newValue } }
    }

    public init(_ teams: [AppTeamInput]? = nil, removals: [AppShareRemovalInput] = []) {
        pending = removals
        current = AppTeamsInput(accountAppUrl: "https://app.cua.ai", state: .ready(teams: teams ?? [
            AppTeamInput(
                id: "o1", alias: "acme", name: "Acme Inc", role: "owner", plan: "teams", seats: 3,
                members: [AppTeamPersonInput(email: "ada@example.com", role: "owner"),
                          AppTeamPersonInput(email: "grace@example.com", role: "member")],
                invites: ["linus@example.com"], paymentFailed: false, paymentFailedUntil: nil),
        ]), currentTeamId: nil)
    }

    public func teams() async throws -> AppTeamsInput { current }
    public func shareRemovals() async throws -> [AppShareRemovalInput] { removals }
    public func ackShareRemovals() async throws { removals = [] }
}

/// Opens an account-app page (Teams, Teams billing). In debug builds `CUA_SPACES_BROWSER` names
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
