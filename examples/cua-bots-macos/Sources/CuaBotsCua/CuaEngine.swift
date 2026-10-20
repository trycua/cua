// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Cua
import CuaBotsCore
import CuaSpaces
import Foundation

/// A custom model endpoint for every bot (`agent_start`'s `base_url`,
/// `model` and `env_from_host`). Unset: each harness's own default and the
/// provider key from this process's environment.
public struct ModelEndpoint: Sendable {
    public var baseURL: String
    public var model: String?
    public var keyVariables: [String]

    public init(baseURL: String, model: String? = nil, keyVariables: [String]) {
        self.baseURL = baseURL
        self.model = model
        self.keyVariables = keyVariables
    }

    /// `CUA_BOTS_MODEL_URL`, `CUA_BOTS_MODEL` and `CUA_BOTS_MODEL_KEY_VARS`
    /// (comma-separated; default `ANTHROPIC_API_KEY,OPENAI_API_KEY`).
    public static func fromEnvironment(_ env: [String: String] = ProcessInfo.processInfo.environment) -> ModelEndpoint? {
        guard let url = env["CUA_BOTS_MODEL_URL"], !url.isEmpty else { return nil }
        let vars = (env["CUA_BOTS_MODEL_KEY_VARS"] ?? "ANTHROPIC_API_KEY,OPENAI_API_KEY")
            .split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
        return ModelEndpoint(baseURL: url, model: env["CUA_BOTS_MODEL"], keyVariables: vars)
    }
}

/// The engine on the cua SDK: each bot gets its own Space (`bot-<name>`), its
/// Volume home is copied into the Space before the agent starts and back after
/// every turn, and pause suspends the Space.
@MainActor
public final class CuaEngine: BotEngine {
    public let cua: Cua
    public let connection: SpacesConnection
    public var endpoint: ModelEndpoint?
    /// The image a new bot's computer runs (`Image.linux()` by default).
    public var image: String?
    /// Also suspend a paused bot's Space (local Spaces only). Off by default:
    /// a paused bot keeps its computer up so the phone can still resume it.
    public var suspendOnPause = false

    private var homes: [String: String] = [:]
    private var clients: [String: SpacesdClient] = [:]
    private var turnText: [String: (turn: UInt32, text: String)] = [:]

    public init(cua: Cua, endpoint: ModelEndpoint? = ModelEndpoint.fromEnvironment()) {
        self.cua = cua
        self.connection = SpacesConnection(cua: cua)
        self.endpoint = endpoint
    }

    // MARK: - Spaces

    public func provision(_ bot: Bot, progress: @escaping @MainActor (EnginePhase) -> Void) async throws -> String {
        progress(.creatingSpace(bot.placement == .local ? "Creating a Space on this Mac" : "Creating a Cua Cloud Space"))
        let options = SpaceCreateOptions(
            image: try image ?? Image.linux(), on: bot.placement.rawValue, kind: "container", runtime: nil,
            name: bot.spaceName, cpus: nil, memoryMb: nil, diskGb: nil, timeoutMs: nil, wait: true,
            reuse: true, command: nil, env: [:], services: [:], spacesd: nil)
        let result = try await cua.spaces().create(options: options)
        guard let info = result.space else {
            throw EngineError.notReady("the Space is still being created (\(result.pendingId ?? "pending"))")
        }
        progress(.startingAgent)
        return info.id
    }

    /// The generated Space handle for a bot.
    public func space(_ bot: Bot) async throws -> CuaSDK.Space {
        guard let id = bot.spaceID else { throw EngineError.noSpace(bot.name) }
        return try await cua.spaces().space(space: id)
    }

    /// The overlay handle, for the live stream and PiP.
    public func streamSpace(_ bot: Bot) async throws -> CuaSpaces.Space {
        guard let id = bot.spaceID else { throw EngineError.noSpace(bot.name) }
        return try await connection.attach(to: SpaceID(id))
    }

    /// Where the bot's home lives inside its Space.
    public func home(_ bot: Bot, _ space: CuaSDK.Space) async throws -> String {
        if let h = homes[bot.id] { return h }
        let guest = (try? await space.home()) ?? "/home/cua"
        let h = "\(guest)/bots/\(bot.id)"
        homes[bot.id] = h
        return h
    }

    // MARK: - Runs

    public func start(_ bot: Bot, volume: VolumeStore, prompt: String) async throws -> String {
        let space = try await space(bot)
        let home = try await home(bot, space)
        try await hydrate(bot, volume: volume, space: space, home: home)
        // #region docs:sw-agent-start
        let options = SpaceAgentOptions(
            envFromHost: endpoint?.keyVariables ?? Self.defaultKeyVariables(bot.harness),
            repo: nil, branch: nil, cwd: home, model: endpoint?.model, baseUrl: endpoint?.baseURL,
            exitWhenIdle: false)
        let report = try await space.agentStart(agent: bot.harness.rawValue, prompt: prompt, show: false,
                                                options: options)
        // #endregion docs:sw-agent-start
        return report.runId
    }

    static func defaultKeyVariables(_ harness: Harness) -> [String] {
        switch harness {
        case .claudeCode: ["ANTHROPIC_API_KEY"]
        case .codex: ["OPENAI_API_KEY"]
        case .hermes, .openclaw: ["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "OPENROUTER_API_KEY"]
        }
    }

    public func send(_ bot: Bot, text: String) async throws {
        guard let run = bot.runID else { throw EngineError.noRun(bot.name) }
        // #region docs:sw-agent-message
        let report = try await space(bot).agentMessage(runId: run, text: text, force: false)
        if !report.ok { throw EngineError.refused(report.reason) }
        // #endregion docs:sw-agent-message
    }

    public func poll(_ bot: Bot, cursor: UInt64) async throws -> (updates: [EngineUpdate], cursor: UInt64) {
        guard let run = bot.runID else { return ([], cursor) }
        let json = try await space(bot).agentEvents(runId: run, cursor: cursor, max: 500)
        let page = try JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any] ?? [:]
        let events = page["events"] as? [[String: Any]] ?? []
        var updates: [EngineUpdate] = []
        for e in events {
            let kind = e["kind"] as? String ?? ""
            let turn = UInt32((e["turn"] as? NSNumber)?.intValue ?? 0)
            switch kind {
            case "message":
                let chunk = e["text"] as? String ?? ""
                var current = turnText[bot.id] ?? (turn, "")
                if current.turn != turn { current = (turn, "") }
                current.text += chunk
                turnText[bot.id] = current
                updates.append(.assistant(turn: turn, text: current.text))
            case "tool_call":
                updates.append(.tool(turn: turn, title: e["tool_title"] as? String ?? "Working"))
            case "turn_ended":
                updates.append(.turnEnded(turn: turn))
                turnText[bot.id] = nil
            case "error":
                updates.append(.failed(e["text"] as? String ?? "The agent reported an error"))
            case "exited":
                updates.append(.failed("The agent stopped: \(e["text"] as? String ?? "exited")"))
            default:
                break
            }
        }
        let next = (page["cursor"] as? NSNumber)?.uint64Value ?? cursor
        return (updates, next)
    }

    public func interrupt(_ bot: Bot) async throws {
        guard let run = bot.runID else { return }
        _ = try await space(bot).agentInterrupt(runId: run)
    }

    // MARK: - The Volume home

    /// Copy `agents/<name>/` from the Volume into the Space.
    func hydrate(_ bot: Bot, volume: VolumeStore, space: CuaSDK.Space, home: String) async throws {
        let prefix = VolumeLayout.home(bot.id)
        for entry in try volume.walk(prefix) {
            guard let data = try volume.read(entry.path) else { continue }
            let rel = String(entry.path.dropFirst(prefix.count))
            if rel.hasPrefix("identity/") { continue }  // the app's records, not the bot's
            _ = try await space.write(path: "\(home)/\(rel)", content: data)
        }
        _ = try await space.bash(command: "mkdir -p '\(home)/outputs' '\(home)/inbox' '\(home)/memory'", timeoutMs: 10_000)
    }

    public func push(_ bot: Bot, volume: VolumeStore) async throws {
        let space = try await space(bot)
        let home = try await home(bot, space)
        let prefix = VolumeLayout.home(bot.id)
        var paths = [VolumeLayout.instructions(bot.id, harness: bot.harness), VolumeLayout.rules(bot.id)]
        paths += try volume.walk(VolumeLayout.inbox(bot.id)).map(\.path)
        for path in paths {
            guard let data = try volume.read(path) else { continue }
            _ = try await space.write(path: "\(home)/\(path.dropFirst(prefix.count))", content: data)
        }
    }

    /// Copy the bot's memory and outputs from the Space back into the Volume.
    public func checkpoint(_ bot: Bot, volume: VolumeStore) async throws {
        let space = try await space(bot)
        let home = try await home(bot, space)
        let list = try await space.bash(
            command: "cd '\(home)' 2>/dev/null && find memory outputs -type f -size -8M 2>/dev/null | head -200",
            timeoutMs: 15_000)
        let staging = FileManager.default.temporaryDirectory
            .appendingPathComponent("cua-bots-\(bot.id)-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: staging) }
        for rel in list.stdout.split(separator: "\n").map(String.init) where !rel.isEmpty {
            let dir = staging.appendingPathComponent((rel as NSString).deletingLastPathComponent)
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            _ = try await space.download(remotePath: "\(home)/\(rel)", destDir: dir.path)
            let local = dir.appendingPathComponent((rel as NSString).lastPathComponent)
            if let data = try? Data(contentsOf: local) {
                // The Volume refuses secrets under agents/; a refused file stays
                // in the Space only.
                try? volume.write(VolumeLayout.home(bot.id) + rel, data)
            }
        }
    }

    // MARK: - Pairing

    /// A link the iPhone app opens to reach this bot's computer directly:
    /// its spacesd address and token. On the same Mac (the iOS Simulator) the
    /// loopback address works; a phone elsewhere goes through the relay.
    public func pairingLink(_ bot: Bot) async throws -> String {
        guard let id = bot.spaceID else { throw EngineError.noSpace(bot.name) }
        let endpoint = try await cua.sandboxes().connect(name: id).service(name: "env").endpoint()
        let token = endpoint.headers.first { $0.name.lowercased().contains("authorization") }?.value
            .replacingOccurrences(of: "Bearer ", with: "") ?? ""
        var c = URLComponents()
        c.scheme = "cuabots"
        c.host = "direct"
        c.queryItems = [URLQueryItem(name: "url", value: endpoint.url), URLQueryItem(name: "token", value: token)]
        return c.string ?? ""
    }

    // MARK: - The pointer

    /// Where the bot's pointer is on its screen, in screen pixels. The app
    /// draws the bot's face there.
    public func pointer(_ bot: Bot) async -> CGPoint? {
        guard let id = bot.spaceID else { return nil }
        do {
            let client: SpacesdClient
            if let c = clients[id] { client = c } else {
                client = try await cua.sandboxes().connect(name: id).spacesd(probeTimeoutMs: 3000)
                clients[id] = client
            }
            let p = try await client.cursorPosition()
            return CGPoint(x: p.x, y: p.y)
        } catch {
            clients[id] = nil
            return nil
        }
    }

    // MARK: - Pause, resume, reset

    public func pause(_ bot: Bot) async throws {
        if let run = bot.runID { _ = try? await space(bot).agentInterrupt(runId: run) }
        guard suspendOnPause, let id = bot.spaceID, bot.placement == .local else { return }
        try await cua.sandboxes().connect(name: id).suspend()
    }

    public func resume(_ bot: Bot) async throws {
        guard suspendOnPause, let id = bot.spaceID, bot.placement == .local else { return }
        try await cua.sandboxes().connect(name: id).resume()
    }

    public func reset(_ bot: Bot) async throws {
        if let run = bot.runID { _ = try? await space(bot).agentStop(runId: run) }
        if let id = bot.spaceID { _ = try await cua.spaces().delete(space: id) }
        homes[bot.id] = nil
    }

    public enum EngineError: LocalizedError {
        case noSpace(String), noRun(String), notReady(String), refused(String)
        public var errorDescription: String? {
            switch self {
            case .noSpace(let n): "\(n) has no computer yet."
            case .noRun(let n): "\(n) isn't running."
            case .notReady(let s): s
            case .refused(let r): r
            }
        }
    }
}
