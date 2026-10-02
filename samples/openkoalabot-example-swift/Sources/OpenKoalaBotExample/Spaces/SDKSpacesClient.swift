// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpaces
import Foundation

/// OpenKoalaBots's `SpacesClient`, implemented on the Cua Spaces SDK.
///
/// This file replaces the 336-line hand-rolled MCP binding that used to live
/// at `Sources/OpenKoalaBotExample/Spaces/MCPSpacesClient.swift`. Everything that was
/// hard about that file — the stdio framing, the `isError` trap, the two-shaped
/// `agent_message` reply, the provider vocabulary, the four cleanup paths, the
/// synchronous transport on the main actor — is now in
/// `libs/spaces-sdk-swift`, shaped by `FRICTION.md`. What is left here is what
/// should always have been left here: the app's own vocabulary, and the
/// mapping into it.
///
/// The mapping is thin on purpose. Where it is *not* thin, the reason is that
/// the app's port predates the SDK and keeps its own view-model types
/// (`SpaceWindow`, `AgentStatus`); a green-field app would use the SDK's
/// `RunSnapshot` and `CuaSpaces.SpaceWindow` directly and delete this file.
final class SDKSpacesClient: SpacesClient {

    /// Failures now arrive on the SDK's error channel. Preserved as a nested
    /// name because the suite catches `SDKSpacesClient.Failure`, and because
    /// `FRICTION.md` §3's guarantee — a tool failure can never be returned as
    /// a value — is exactly what this type is.
    typealias Failure = SpacesError

    private let connection: SpacesConnection
    private var attached: [SpaceID: Space] = [:]
    private let lock = NSLock()

    let capabilities = SpacesCapabilities(
        spaceLifecycle: true,
        agents: true,
        windowList: true,
        // Still false, and still for the reason §10 gives: *this* port does not
        // render anything. The difference the SDK makes is that the frames now
        // have a supported home — `Space.streamEndpoint()` plus
        // `CuaSpacesStreaming` — rather than no home at all. Claiming live
        // pixels here would put them behind `presentScreen`, which is exactly
        // the conflation §10 is about.
        liveScreenPixels: false,
        upload: true,
        download: true,
        notes: [
            "Backed by CuaSpaces (libs/spaces-sdk-swift), not a hand-rolled MCP binding.",
            "The SDK owns the JSON-RPC framing, so FRICTION.md §1's silent response "
            + "desync cannot recur: no caller ever sees the read buffer.",
            "Tool failures are thrown, never returned (FRICTION.md §3).",
            "Space.streamEndpoint() hands back frames for an in-app surface, separately "
            + "from the operator-facing display tools (FRICTION.md §10).",
        ])

    /// Where the Spaces runtime lives. There is no Python server any more:
    /// every tool is the Rust `cua-spaces` implementation, either in this
    /// process or in a running `cua daemon`.
    enum Backend: Sendable {
        /// The Spaces runtime in this process. `spacesHome` is the registry
        /// directory (default `~/.cua`); `teleportHome` makes teleport read
        /// app sessions under that directory instead of the real host, where
        /// teleport is available (it ships with Cua Spaces; this embedded
        /// runtime refuses it with `HostCapabilityMissing`).
        case embedded(spacesHome: String? = nil, teleportHome: String? = nil)
        /// A running `cua daemon` (`nil` address: its discovery file).
        case daemon(address: String? = nil, token: String? = nil)
        /// The daemon when one runs, otherwise embedded (`Spaces.local()`).
        case automatic
    }

    /// The backend the environment asks for: `OPENKOALABOTS_SPACES=daemon`,
    /// `embedded`, or unset for `automatic`. `OPENKOALABOTS_SPACES_HOME` points
    /// an embedded registry somewhere other than `~/.cua`.
    static func backendFromEnvironment(
        _ env: [String: String] = ProcessInfo.processInfo.environment
    ) -> Backend {
        let home = env["OPENKOALABOTS_SPACES_HOME"].flatMap { $0.isEmpty ? nil : $0 }
        switch env["OPENKOALABOTS_SPACES"] {
        case "daemon": return .daemon(address: env["CUA_DAEMON"], token: env["CUA_DAEMON_TOKEN"])
        case "embedded": return .embedded(spacesHome: home)
        default: return home.map { .embedded(spacesHome: $0) } ?? .automatic
        }
    }

    init(backend: Backend = .automatic) throws {
        // #region docs:sw-connect
        switch backend {
        case let .embedded(spacesHome, teleportHome):
            connection = try SpacesConnection.embedded(spacesHome: spacesHome,
                                                       teleportHome: teleportHome)
        case let .daemon(address, token):
            connection = try SpacesConnection.daemon(address: address, token: token)
        case .automatic:
            connection = try Spaces.local()
        }
        // #endregion docs:sw-connect
    }

    /// Wrap a connection the caller already holds (tests, the scenario runner).
    init(connection: SpacesConnection) {
        self.connection = connection
    }

    /// The SDK connection, for callers that want the real API.
    var sdkConnection: SpacesConnection { connection }

    /// Register a machine that already runs cua-spacesd and return its id.
    /// Never creates one.
    // #region docs:sw-add
    func addSpace(url: String, token: String?, name: String?) async throws -> String {
        try await connection.add(url: url, token: token, name: name).id.rawValue
    }
    // #endregion docs:sw-add

    // MARK: - Pinning

    /// Environment override: attach to an already-warm Space instead of
    /// creating one.
    ///
    /// The SDK made this a *convenience* rather than the only way to say "use
    /// this one" — `SpacesConnection.attach(to:)` takes a `SpaceID` and
    /// `createSpace(options:)` takes where it runs as a required argument,
    /// which is what §5, §28 and §33 all asked for. The variable survives because the suite and the app
    /// shell are both driven from the environment.
    static let spaceOverrideVariable = "OPENKOALABOTS_TEST_SPACE"

    /// Set in-process by a caller that has just registered the Space it means
    /// (the live test harness, the scenario runner). Wins over the variable,
    /// which Foundation snapshots at first read (`FRICTION.md` §57).
    nonisolated(unsafe) static var pinnedSpace: String?

    static var overriddenSpace: String? {
        if let pinnedSpace { return pinnedSpace }
        guard let v = ProcessInfo.processInfo.environment[spaceOverrideVariable] else { return nil }
        let t = v.trimmingCharacters(in: .whitespacesAndNewlines)
        return t.isEmpty ? nil : t
    }

    // MARK: - Handles

    /// A `Space` handle, cached so the id is resolved once rather than on every
    /// call (§7: the raw protocol restates the Space id everywhere).
    private func space(_ id: String) async throws -> Space {
        let sid = SpaceID(id)
        lock.lock()
        let cached = attached[sid]
        lock.unlock()
        if let cached { return cached }
        let handle = try await connection.attach(to: sid)
        lock.lock()
        attached[sid] = handle
        lock.unlock()
        return handle
    }

    /// The SDK handle for a Space, for callers that want the real API rather
    /// than this app's port. `AppModel` uses it for the live stream.
    func sdkSpace(_ id: String) async throws -> Space {
        try await space(id)
    }

    // MARK: - Escape hatches

    @discardableResult
    func handshake() async throws -> [String] {
        try await connection.availableTools()
    }

    /// Call a tool the app's port does not model. Async, because the SDK is
    /// async all the way down (§30) and this app is not going to block the
    /// thread that draws.
    @discardableResult
    func raw(_ name: String, _ args: [String: Any]) async throws -> Any {
        let converted = args.mapValues { JSONValue(any: $0) }
        return try await connection.callTool(name, converted).foundationObject
    }

    // MARK: - Sandboxes

    func listSpaces() async throws -> [SpaceSummary] {
        try await connection.spaces().map {
            SpaceSummary(id: $0.id.rawValue, provider: $0.provider.rawValue,
                         os: $0.operatingSystem, phase: $0.rawPhase, ip: $0.ipAddress)
        }
    }

    func ensureSpace() async throws -> String {
        if let pinned = Self.overriddenSpace {
            // `attach(to:)`, so a pinned Space that is missing is an error and
            // never a silently created sandbox, so §33's bug cannot recur.
            return try await connection.attach(to: SpaceID(pinned)).id.rawValue
        }
        // Get-or-create in the user's default location (`cua config set
        // default.on`), said out loud rather than left to the backend.
        return try await connection.createSpace(
            on: SpacePlacement.configuredDefault.location, reuse: true).id.rawValue
    }

    func createSpace(on: SpacePlacement, image: String?, wait: Bool) async throws -> SpaceSummary {
        let s = try await connection.createSpace(on: on.location, image: image, wait: wait)
        return SpaceSummary(id: s.id.rawValue, provider: s.provider.rawValue,
                            os: s.info.operatingSystem, phase: s.info.rawPhase,
                            ip: s.info.ipAddress)
    }

    func deleteSpace(_ space: String) async throws {
        try await connection.deleteSpace(SpaceID(space))
    }

    // MARK: - Agent threads

    /// Whether a started run gets a Terminal window on the Space's desktop.
    ///
    /// On for a person using the app — watching an agent work is the point of
    /// having a Space with a screen. **Off for the test suite**, which sets
    /// `OPENKOALABOTS_AGENT_WINDOWS=0`: a suite that starts dozens of runs
    /// otherwise leaves dozens of terminals behind, and the cheapest cleanup to
    /// get right is the one where nothing was created. `FRICTION.md` §54.
    static let agentWindowsVariable = "OPENKOALABOTS_AGENT_WINDOWS"

    /// Seeded from the environment once, and settable afterwards.
    ///
    /// A stored property rather than one that reads `ProcessInfo` on every
    /// call, because of a trap worth keeping written down: Foundation
    /// snapshots `ProcessInfo.processInfo.environment` the first time it is
    /// read, so a `setenv` made later — by a test bundle arming itself, say —
    /// is never visible through it. The suite set `OPENKOALABOTS_AGENT_WINDOWS=0`
    /// in its own process, the value never arrived, and every run went on
    /// opening a Terminal window on the demo machine. `FRICTION.md` §57.
    nonisolated(unsafe) static var showsAgentWindows: Bool = {
        ProcessInfo.processInfo.environment[agentWindowsVariable] != "0"
    }()

    /// A custom model endpoint for every run this app starts, or `nil` for
    /// the harness default. `OPENKOALABOTS_MODEL_URL` (plus optional
    /// `OPENKOALABOTS_MODEL` and `OPENKOALABOTS_MODEL_KEY_VAR`, default
    /// `ANTHROPIC_API_KEY`, forwarded from this process's environment) sets
    /// it; the scenario runner sets it directly.
    nonisolated(unsafe) static var agentEndpoint: AgentEndpoint? = {
        let env = ProcessInfo.processInfo.environment
        guard let url = env["OPENKOALABOTS_MODEL_URL"], !url.isEmpty else { return nil }
        return AgentEndpoint(baseURL: url, model: env["OPENKOALABOTS_MODEL"],
                             envFromHost: [env["OPENKOALABOTS_MODEL_KEY_VAR"] ?? "ANTHROPIC_API_KEY"])
    }()

    func startBot(space: String, bot: Bot, prompt: String) async throws -> AgentRun {
        let handle = try await self.space(space)
        // #region docs:sw-agent-start
        let run = try await handle.startAgent(AgentStartRequest(
            prompt: prompt, showsWindow: Self.showsAgentWindows, endpoint: Self.agentEndpoint))
        // #endregion docs:sw-agent-start
        return AgentRun(runID: run.id.rawValue, agent: run.agent, space: space,
                        capabilities: run.turnModel?.published ?? [:], notes: run.notes)
    }

    @discardableResult
    func message(space: String, runID: String, text: String, force: Bool) async throws -> MessageOutcome {
        let handle = try await self.space(space)
        // #region docs:sw-agent-message
        let delivery = try await handle.run(RunID(runID))
            .send(text, mode: force ? .interruptCurrentTurn : .refuseIfBusy)
        // #endregion docs:sw-agent-message
        // One shape, one populated `reason`, on both branches (§4).
        return MessageOutcome(accepted: delivery.accepted, reason: delivery.reason)
    }

    func status(space: String, runID: String, tail: Int) async throws -> AgentStatus {
        let handle = try await self.space(space)
        return Self.status(from: try await handle.run(RunID(runID)).status(tail: tail))
    }

    func events(space: String, runID: String, cursor: UInt64) async throws -> String {
        let handle = try await self.space(space)
        if let native = try await handle.native() {
            return try await native.agentEvents(runId: runID, cursor: cursor, max: 500)
        }
        let page = try await raw("agent_events", ["space": space, "run_id": runID,
                                                  "cursor": cursor, "max": 500])
        return String(decoding: try JSONSerialization.data(withJSONObject: page), as: UTF8.self)
    }

    @discardableResult
    func stopBot(space: String, runID: String) async throws -> StopOutcome {
        let handle = try await self.space(space)
        let outcome = try await handle.run(RunID(runID)).stop()
        return StopOutcome(stopped: outcome.stopped, alive: outcome.alive, reason: outcome.reason)
    }

    func listBots(space: String) async throws -> [AgentRunSummary] {
        let handle = try await self.space(space)
        return try await handle.runs().map {
            AgentRunSummary(id: $0.id.rawValue, agent: $0.agent,
                            state: AgentState(rawValue: $0.state.rawValue) ?? .unknown,
                            summary: $0.summary, acceptsMessage: $0.acceptsMessage,
                            createdAt: $0.createdAt?.timeIntervalSince1970)
        }
    }

    /// Remove a whole run — process, run directory, LaunchAgent plist and
    /// Terminal window — in one call (§8). The suite's four-path teardown is
    /// now this.
    @discardableResult
    func deleteRun(space: String, runID: String) async throws -> RunCleanup {
        try await self.space(space).run(RunID(runID)).delete()
    }

    // MARK: - Windows

    func windows(space: String) async throws -> [SpaceWindow] {
        try await self.space(space).windows().map {
            SpaceWindow(id: $0.id.rawValue, app: $0.app, title: $0.title,
                        width: Int($0.pixelSize.width), height: Int($0.pixelSize.height),
                        visible: $0.visible)
        }
    }

    // MARK: - Files

    func upload(space: String, localPath: String, remotePath: String) async throws {
        _ = try await self.space(space).upload(
            URL(fileURLWithPath: localPath), to: .exactPath(remotePath))
    }

    @discardableResult
    func download(space: String, remotePath: String, localDirectory: String?) async throws -> String {
        try await self.space(space)
            .download(remotePath, into: localDirectory.map { URL(fileURLWithPath: $0) })
            .path
    }

    // MARK: - Presentation

    func presentScreen(space: String, window: SpaceWindow, tier: ComputerTier) async throws {
        // Still deliberately inert here, but for a different reason than before.
        // The SDK now separates the two things §10 says were conflated:
        // `Space.present(_:)` draws on the *operator's* machine, and
        // `Space.streamEndpoint()` hands this app frames for its own surface.
        // The tiers want the second, so they go through `CuaSpacesStreaming`
        // and this method has nothing left to do.
    }

    // MARK: - Decoding

    /// Map the SDK's one state type onto the app's view model.
    ///
    /// §22 is answered inside the SDK: `RunSnapshot` is what both the roster
    /// call and the detail call return, and the cheap one no longer blanks the
    /// reason the expensive one supplied.
    static func status(from snapshot: RunSnapshot) -> AgentStatus {
        AgentStatus(state: AgentState(rawValue: snapshot.state.rawValue) ?? .unknown,
                    reason: snapshot.reason,
                    acceptsMessage: snapshot.acceptsMessage,
                    exitCode: snapshot.exitCode,
                    summary: snapshot.summary,
                    tail: snapshot.outputTail ?? "")
    }

    /// Decode a raw `agent_status` payload. Kept because the suite pins the
    /// status vocabulary through it (§9: `unknown` means the probe failed, and
    /// is never a stand-in for a guess).
    static func decodeStatus(_ d: [String: Any]) -> AgentStatus {
        let json = JSONValue(any: d).objectValue ?? [:]
        return status(from: RunSnapshot.decodeForClient(json))
    }
}

extension RunSnapshot {
    /// The app-side entry point into the SDK's decoder, for a payload that
    /// arrived outside a `Space` handle.
    static func decodeForClient(_ d: [String: JSONValue]) -> RunSnapshot {
        RunSnapshot(id: RunID(d["run_id"]?.stringValue ?? ""),
                    space: SpaceID(d["space"]?.stringValue ?? ""),
                    agent: d["agent"]?.stringValue ?? "",
                    state: CuaSpaces.AgentState(wire: d["status"]?.stringValue),
                    reason: d["reason"]?.stringValue ?? "",
                    acceptsMessage: d["accepts_message"]?.boolValue ?? false,
                    exitCode: d["exit_code"]?.intValue,
                    summary: RunMetadata.strip(d["summary"]?.stringValue ?? ""),
                    outputTail: d["output_tail"]?.stringValue)
    }
}
