import Foundation

// MARK: - Persistent agents

/// A named agent whose memory outlives its runs and its Space: its harness
/// keeps its memory in its home in the Cua Volume (`agents/<name>/`), which the
/// daemon restores before each run and saves after each turn.
public struct PersistentAgent: Sendable, Hashable {
    public let name: String
    /// Harness id (`claude-code`, `hermes`, `openclaw`, `openai-codex`, ...).
    public let harness: String
    public let space: SpaceID
    public let isPaused: Bool
    /// `running`, `suspended` (a paused local Space) or `released` (a paused
    /// cloud Space, created again on resume).
    public let spaceState: String
    public let runID: RunID?
    /// When its home was last saved to the drive.
    public let savedAt: Date?
    public let lastError: String?

    init(_ d: [String: JSONValue]) {
        name = d["name"]?.stringValue ?? ""
        harness = d["harness"]?.stringValue ?? ""
        space = SpaceID(d["space"]?.stringValue ?? "")
        isPaused = d["paused"]?.boolValue ?? false
        spaceState = d["space_state"]?.stringValue ?? "running"
        runID = d["run_id"]?.stringValue.map(RunID.init)
        savedAt = PersistentAgent.date(ms: d["saved_ms"]?.doubleValue)
        lastError = d["last_error"]?.stringValue
    }

    static func date(ms: Double?) -> Date? {
        guard let ms, ms > 0 else { return nil }
        return Date(timeIntervalSince1970: ms / 1000)
    }
}

/// What moved between the drive and a Space.
public struct HomeTransfer: Sendable, Hashable {
    public let files: Int
    public let bytes: Int
    public let milliseconds: Int
    /// `path (kind)` of each file the secret scanner kept out of the drive.
    public let blocked: [String]

    init(_ d: [String: JSONValue]) {
        files = d["files"]?.intValue ?? 0
        bytes = d["bytes"]?.intValue ?? 0
        milliseconds = d["millis"]?.intValue ?? 0
        blocked = (d["blocked"]?.arrayValue ?? []).compactMap { pair in
            guard let p = pair.arrayValue, p.count == 2,
                  let path = p[0].stringValue, let kind = p[1].stringValue else { return nil }
            return "\(path) (\(kind))"
        }
    }
}

/// A recurring turn of a persistent agent, fired by the daemon whether or not
/// an app is open.
public struct AgentRoutine: Sendable, Hashable, Identifiable {
    public let id: String
    public let agent: String
    public let title: String
    public let prompt: String
    /// `Every day at 8:00 AM`.
    public let label: String
    public let isEnabled: Bool
    public let nextFire: String?
    public let lastOutcome: String?

    init(_ d: [String: JSONValue]) {
        id = d["id"]?.stringValue ?? ""
        agent = d["botID"]?.stringValue ?? ""
        title = d["title"]?.stringValue ?? ""
        prompt = d["prompt"]?.stringValue ?? ""
        label = d["label"]?.stringValue ?? ""
        isEnabled = d["isEnabled"]?.boolValue ?? true
        nextFire = d["next_fire"]?.stringValue
        lastOutcome = d["lastOutcome"]?.stringValue
    }
}

/// One persistent agent's access to one of the user's computers.
public struct ComputerGrant: Sendable, Hashable, Identifiable {
    public let id: String
    public let agent: String
    public let machine: SpaceID
    public let isRevoked: Bool

    init(_ d: [String: JSONValue]) {
        id = d["id"]?.stringValue ?? ""
        agent = d["agent"]?.stringValue ?? ""
        machine = SpaceID(d["machine"]?.stringValue ?? "")
        isRevoked = d["revoked"]?.boolValue ?? false
    }
}

/// Persistent agents, their routines and their access to your computers.
public struct PersistentAgents: Sendable {
    let connection: SpacesConnection

    private func agent(_ d: [String: JSONValue]) -> PersistentAgent { PersistentAgent(d) }

    /// `persistent_agent_create`.
    @discardableResult
    public func create(_ name: String, harness: String, in space: SpaceID,
                       model: String? = nil, baseURL: String? = nil,
                       envFromHost: [String] = [],
                       env: [String: String] = [:]) async throws -> PersistentAgent {
        var args: [String: JSONValue] = [
            "name": .string(name), "agent": .string(harness), "space": .string(space.rawValue),
            "env_from_host": .array(envFromHost.map { .string($0) }),
            "env": .object(env.mapValues { .string($0) }),
        ]
        if let model { args["model"] = .string(model) }
        if let baseURL { args["base_url"] = .string(baseURL) }
        return agent(try await connection.object("persistent_agent_create", args))
    }

    /// `persistent_agent_list`.
    public func list() async throws -> [PersistentAgent] {
        try await connection.array("persistent_agent_list", [:], unwrapping: "agents")
            .compactMap(\.objectValue).map(PersistentAgent.init)
    }

    /// `persistent_agent_remove`. The home stays in the drive.
    public func remove(_ name: String) async throws {
        _ = try await connection.object("persistent_agent_remove", ["name": .string(name)])
    }

    /// `persistent_agent_send`: a follow-up to the agent's idle run, or a new
    /// run with its home restored. Returns the run and whether it is new.
    @discardableResult
    public func send(_ text: String, to name: String) async throws -> (run: RunID, started: Bool) {
        let d = try await connection.object("persistent_agent_send",
                                            ["name": .string(name), "text": .string(text)])
        return (RunID(d["run_id"]?.stringValue ?? ""), d["started"]?.boolValue ?? false)
    }

    /// `persistent_agent_save`.
    @discardableResult
    public func save(_ name: String) async throws -> HomeTransfer {
        HomeTransfer(try await connection.object("persistent_agent_save", ["name": .string(name)]))
    }

    /// `agent_pause`: its run, its routines, and its Space. Returns the Space's
    /// state (`suspended`, `released` or `running`).
    @discardableResult
    public func pause(_ name: String) async throws -> String {
        let d = try await connection.object("agent_pause", ["name": .string(name)])
        return d["space_state"]?.stringValue ?? "running"
    }

    /// `agent_resume`. Returns the Space (a released cloud Space gets a new
    /// one) and how long until the home was back.
    @discardableResult
    public func resume(_ name: String, prompt: String? = nil) async throws
        -> (space: SpaceID, readyMilliseconds: Int) {
        var args: [String: JSONValue] = ["name": .string(name)]
        if let prompt { args["prompt"] = .string(prompt) }
        let d = try await connection.object("agent_resume", args)
        return (SpaceID(d["space"]?.stringValue ?? ""), d["ready_ms"]?.intValue ?? 0)
    }

    /// `routine_add`.
    @discardableResult
    public func addRoutine(for agent: String, title: String, prompt: String,
                           schedule: RoutineSchedule) async throws -> AgentRoutine {
        var args: [String: JSONValue] = [
            "agent": .string(agent), "title": .string(title), "prompt": .string(prompt),
        ]
        let days = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"]
        let hm = { (h: Int, m: Int) in String(format: "%02d:%02d", h, m) }
        switch schedule {
        case .everyMinutes(let m): args["every_minutes"] = .number(Double(m))
        case .dailyAt(let h, let m): args["daily_at"] = .string(hm(h, m))
        case .weeklyOn(let wd, let h, let m):
            args["weekly_on"] = .string("\(days[max(0, min(6, wd - 1))]) \(hm(h, m))")
        }
        return AgentRoutine(try await connection.object("routine_add", args))
    }

    /// `routine_list`.
    public func routines(of agent: String? = nil) async throws -> [AgentRoutine] {
        var args: [String: JSONValue] = [:]
        if let agent { args["agent"] = .string(agent) }
        return try await connection.array("routine_list", args, unwrapping: "routines")
            .compactMap(\.objectValue).map(AgentRoutine.init)
    }

    /// `routine_remove`.
    public func removeRoutine(_ id: String) async throws {
        _ = try await connection.object("routine_remove", ["id": .string(id)])
    }

    /// `routine_set_enabled`.
    @discardableResult
    public func setRoutine(_ id: String, enabled: Bool) async throws -> AgentRoutine {
        AgentRoutine(try await connection.object(
            "routine_set_enabled", ["id": .string(id), "enabled": .bool(enabled)]))
    }

    /// `computer_access_grant`: let one agent use one of your computers
    /// (the user confirms with Touch ID or the passphrase).
    @discardableResult
    public func allowComputer(_ machine: SpaceID, for agent: String) async throws -> ComputerGrant {
        ComputerGrant(try await connection.object(
            "computer_access_grant",
            ["agent": .string(agent), "machine": .string(machine.rawValue)]))
    }

    /// `computer_access_revoke`. Returns how many grants.
    @discardableResult
    public func revokeComputer(_ machine: SpaceID? = nil, for agent: String) async throws -> Int {
        var args: [String: JSONValue] = ["agent": .string(agent)]
        if let machine { args["machine"] = .string(machine.rawValue) }
        return try await connection.object("computer_access_revoke", args)["revoked"]?.intValue ?? 0
    }

    /// `computer_access_list`.
    public func computerAccess(for agent: String? = nil) async throws -> [ComputerGrant] {
        var args: [String: JSONValue] = [:]
        if let agent { args["agent"] = .string(agent) }
        return try await connection.array("computer_access_list", args, unwrapping: "grants")
            .compactMap(\.objectValue).map(ComputerGrant.init)
    }
}

// MARK: - Notifications

/// One entry of the notifications feed the daemon keeps.
public struct AgentNotification: Sendable, Hashable, Identifiable {
    public let id: String
    public let at: Date
    public let agent: String?
    /// `turn_ended`, `message`, `approval` or `error`.
    public let kind: String
    public let title: String
    public let body: String
    public let isRead: Bool

    init(_ d: [String: JSONValue]) {
        id = d["id"]?.stringValue ?? ""
        at = PersistentAgent.date(ms: d["at_ms"]?.doubleValue) ?? Date(timeIntervalSince1970: 0)
        agent = d["agent"]?.stringValue
        kind = d["kind"]?.stringValue ?? ""
        title = d["title"]?.stringValue ?? ""
        body = d["body"]?.stringValue ?? ""
        isRead = d["read"]?.boolValue ?? false
    }
}

/// The notifications feed: turn ends ("Your research is ready"), agents'
/// `notify_user` calls, access requests and failures.
public struct Notifications: Sendable {
    let connection: SpacesConnection

    /// `notify_user`.
    @discardableResult
    public func post(_ title: String, body: String = "", agent: String? = nil) async throws -> String {
        var args: [String: JSONValue] = ["title": .string(title), "body": .string(body)]
        if let agent { args["agent"] = .string(agent) }
        return try await connection.object("notify_user", args)["id"]?.stringValue ?? ""
    }

    /// `notifications_list`, newest first.
    public func list(unreadOnly: Bool = false, since: Date? = nil) async throws -> [AgentNotification] {
        var args: [String: JSONValue] = ["unread_only": .bool(unreadOnly)]
        if let since { args["since_ms"] = .number((since.timeIntervalSince1970 * 1000).rounded()) }
        return try await connection.array("notifications_list", args, unwrapping: "notifications")
            .compactMap(\.objectValue).map(AgentNotification.init)
    }

    /// `notifications_ack`: mark read (every one when `ids` is empty).
    @discardableResult
    public func markRead(_ ids: [String] = []) async throws -> Int {
        try await connection.object("notifications_ack",
                                    ["ids": .array(ids.map { .string($0) })])["marked"]?.intValue ?? 0
    }
}

extension SpacesConnection {
    /// Persistent agents, their routines and their access to your computers.
    public var persistentAgents: PersistentAgents { PersistentAgents(connection: self) }
    /// The notifications feed.
    public var notifications: Notifications { Notifications(connection: self) }
}
