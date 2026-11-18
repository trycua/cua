import Foundation

// The honesty apparatus.
//
// Every type in this file exists so that a product can *render the truth* about
// what the backend actually does, and so that the day the backend gains the
// feature, the boolean flips and no call site breaks.
//
// The rule these types enforce: the SDK never claims a capability it does not
// have, and never quietly upgrades a guess into a fact. Where a feature is
// client-side, it says `isServerBacked == false`. Where a constant is the
// SDK's rather than the server's, it says `isServerPublished == false`. Where
// an event was inferred from scrollback rather than published by the harness,
// it says `isInferred == true`.

// MARK: - Which agent backends are real

/// Which agent harness to start.
///
/// Replaces a bare `agent: String`. Two backends are production-ready; the
/// others are stubs in the Spaces harness, and a caller learns which at the
/// call site rather than by watching a run fail.
public struct AgentKind: Sendable, Hashable, Codable, ExpressibleByStringLiteral,
                         CustomStringConvertible {
    public let rawValue: String
    public init(_ rawValue: String) { self.rawValue = rawValue }
    public init(stringLiteral value: String) { self.init(value) }
    public var description: String { rawValue }

    public static let claudeCode = AgentKind("claude-code")
    public static let codex = AgentKind("codex")

    /// The two harnesses that are wired end to end. Everything else that
    /// `agent_start` will accept is a stub that starts and exits.
    public static let productionReady: Set<AgentKind> = [.claudeCode, .codex]

    /// Whether this backend is wired end to end, **reported rather than
    /// discovered by failing**. A picker renders the rest as unavailable.
    public var isProductionReady: Bool { AgentKind.productionReady.contains(self) }

    public init(from decoder: Decoder) throws {
        self.init(try decoder.singleValueContainer().decode(String.self))
    }
    public func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        try c.encode(rawValue)
    }
}

// MARK: - Approvals

/// What a human decided at an approval seam.
public enum ApprovalDecision: Sendable, Hashable { case allowOnce, deny, alwaysAllow }

// MARK: - Scheduling, client-side and saying so

/// How often a schedule fires.
public enum Cadence: Sendable, Hashable, Codable {
    case every(seconds: Double)
    case dailyAt(hour: Int, minute: Int)
    case weekdaysAt(hour: Int, minute: Int)

    public static func every(_ duration: Duration) -> Cadence {
        .every(seconds: Double(duration.components.seconds)
            + Double(duration.components.attoseconds) / 1e18)
    }
}

/// What to do when the process was asleep across several due slots.
public enum MissedSlotPolicy: Sendable, Hashable {
    /// Twelve overnight routines landing at once is worse than a skipped run.
    case collapseToOneFiring
}

/// A recurring piece of work.
public struct ScheduledAgent: Sendable, Hashable, Identifiable, Codable {
    public struct ID: SpacesIdentifier, Codable {
        public let rawValue: String
        public init(_ rawValue: String) { self.rawValue = rawValue }
    }

    public let id: ID
    public var title: String
    public var prompt: String
    public var agent: AgentKind
    public var cadence: Cadence
    public var metadata: [String: String]
    public internal(set) var lastFired: Date?
    public internal(set) var lastRun: RunID?

    public init(id: ID = ID(UUID().uuidString), title: String, prompt: String,
                agent: AgentKind = .claudeCode, cadence: Cadence,
                metadata: [String: String] = [:]) {
        self.id = id
        self.title = title
        self.prompt = prompt
        self.agent = agent
        self.cadence = cadence
        self.metadata = metadata
    }
}

/// Where schedules are persisted between launches.
public protocol ScheduleStore: Sendable {
    func load() async throws -> [ScheduledAgent]
    func save(_ schedules: [ScheduledAgent]) async throws
}

/// An in-memory store. The default, because a library may not decide where a
/// product's durable state lives.
public actor InMemoryScheduleStore: ScheduleStore {
    private var schedules: [ScheduledAgent] = []
    public init() {}
    public func load() async throws -> [ScheduledAgent] { schedules }
    public func save(_ schedules: [ScheduledAgent]) async throws { self.schedules = schedules }
}

/// Recurring agent work.
///
/// **This fires only while your process is running.** There is no scheduler in
/// the Spaces backend: `spaces_mcp.py` contains zero occurrences of `schedule`,
/// `cron` or `recurr`. So this is a durable *record* the SDK persists plus a
/// tick loop the SDK owns — which is strictly better than every client writing
/// its own, and strictly worse than a server that fires while the laptop is
/// shut. `isServerBacked` publishes which of those you are getting.
///
/// When a backend gains one, `isServerBacked` flips and nothing at the call
/// site changes.
public actor Scheduler {

    /// `false` in every shipping backend. **Render it.** A product that draws a
    /// routine as though it were durable is lying on the SDK's behalf.
    public static let isServerBacked = false

    /// The documented policy, so every client does not invent a different one.
    public static let missedSlotPolicy: MissedSlotPolicy = .collapseToOneFiring

    private let space: Space
    private let store: ScheduleStore
    private var schedules: [ScheduledAgent] = []
    private var tick: Task<Void, Never>?
    private var firings: [(ScheduledAgent.ID, RunID)] = []

    init(space: Space, store: ScheduleStore) {
        self.space = space
        self.store = store
    }

    public func load() async throws { schedules = try await store.load() }

    @discardableResult
    public func add(_ schedule: ScheduledAgent) async throws -> ScheduledAgent {
        schedules.removeAll { $0.id == schedule.id }
        schedules.append(schedule)
        try await store.save(schedules)
        return schedule
    }

    public func remove(_ id: ScheduledAgent.ID) async throws {
        schedules.removeAll { $0.id == id }
        try await store.save(schedules)
    }

    public func all() -> [ScheduledAgent] { schedules }

    /// Runs this scheduler caused, so a history view has the link back that a
    /// prompt marker used to carry.
    public func firedRuns(of id: ScheduledAgent.ID) -> [RunID] {
        firings.filter { $0.0 == id }.map(\.1)
    }

    /// Begin firing. Dies with the process — see `isServerBacked`.
    public func start(checkingEvery interval: Duration = .seconds(30)) {
        guard tick == nil else { return }
        tick = Task { [weak self] in
            while !Task.isCancelled {
                await self?.fireDue()
                try? await Task.sleep(for: interval)
            }
        }
    }

    public func stop() {
        tick?.cancel()
        tick = nil
    }

    private func fireDue() async {
        let now = Date()
        for (index, schedule) in schedules.enumerated() where Scheduler.isDue(schedule, at: now) {
            // `.collapseToOneFiring`: one firing however many slots elapsed.
            guard let run = try? await space.startAgent(
                prompt: schedule.prompt,
                agent: schedule.agent.rawValue,
                metadata: schedule.metadata.merging(["cua.schedule": schedule.id.rawValue]) { a, _ in a })
            else { continue }
            schedules[index].lastFired = now
            schedules[index].lastRun = run.id
            firings.append((schedule.id, run.id))
        }
        try? await store.save(schedules)
    }

    static func isDue(_ schedule: ScheduledAgent, at now: Date,
                      calendar: Calendar = .current) -> Bool {
        switch schedule.cadence {
        case let .every(seconds):
            guard let last = schedule.lastFired else { return true }
            return now.timeIntervalSince(last) >= seconds
        case let .dailyAt(hour, minute), let .weekdaysAt(hour, minute):
            let parts = calendar.dateComponents([.hour, .minute, .weekday], from: now)
            if case .weekdaysAt = schedule.cadence,
               let weekday = parts.weekday, weekday == 1 || weekday == 7 { return false }
            guard (parts.hour ?? -1) == hour, (parts.minute ?? -1) >= minute else { return false }
            guard let last = schedule.lastFired else { return true }
            return !calendar.isDate(last, inSameDayAs: now)
        }
    }
}

extension Space {
    /// A client-side scheduler for this Space. See `Scheduler.isServerBacked`.
    public func scheduler(store: ScheduleStore = InMemoryScheduleStore()) -> Scheduler {
        Scheduler(space: self, store: store)
    }
}
