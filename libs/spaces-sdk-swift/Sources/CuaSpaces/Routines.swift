import Combine
import Foundation

/// A **routine**: a recurring task one Bot runs on a schedule.
///
/// A routine belongs to exactly one Bot, because a Bot is a persistent coworker
/// with one long-lived thread; a routine firing is that Bot being given another
/// turn, not a new conversation. The same model as
/// `@trycua/cua/spaces/routines` and `cua_spaces::routines`, and the same
/// saved JSON.
public struct Routine: Identifiable, Codable, Hashable, Sendable {
    public var id: String = UUID().uuidString
    /// The Bot that runs it. Routines never span Bots; a group chat does that.
    public var botID: String
    /// What the user calls it in the list.
    public var title: String
    /// The text handed to the Bot when the routine fires. This is the whole
    /// behaviour: a routine is a saved prompt plus a clock.
    public var prompt: String
    public var schedule: RoutineSchedule
    public var isEnabled: Bool = true
    public var createdAt: Date = Date()

    // MARK: Firing history, persisted so a restart does not re-fire the past.

    /// When the scheduler last actually started a run for this routine.
    public var lastFiredAt: Date? = nil
    /// The agent run the last firing produced, so the UI can point at it.
    public var lastRunID: String? = nil
    /// The scheduler's own words about the last firing: started, refused, failed.
    public var lastOutcome: String? = nil

    public init(id: String = UUID().uuidString, botID: String, title: String, prompt: String,
                schedule: RoutineSchedule, isEnabled: Bool = true, createdAt: Date = Date()) {
        self.id = id
        self.botID = botID
        self.title = title
        self.prompt = prompt
        self.schedule = schedule
        self.isEnabled = isEnabled
        self.createdAt = createdAt
    }

    /// Marks a routine-originated turn in the transcript.
    public static let prefix = "[routine]"

    /// The text a runner hands the Bot: `[routine] <title>: <prompt>`.
    public var turnText: String { "\(Self.prefix) \(title): \(prompt)" }

    /// When this routine should next fire, given when it last did.
    ///
    /// `reference` is deliberately a parameter rather than `Date()`: the whole
    /// scheduler is testable only if "now" can be supplied.
    public func nextFireDate(after reference: Date, calendar: Calendar = .current) -> Date? {
        guard isEnabled else { return nil }
        return schedule.nextFireDate(after: max(reference, lastFiredAt ?? .distantPast),
                                     calendar: calendar)
    }

    /// Whether the scheduler should fire this routine at `now`.
    ///
    /// The comparison is against the *last firing*, not against the tick, so a
    /// scheduler that was asleep (app closed, machine suspended) fires once on
    /// waking rather than once per missed interval.
    public func isDue(at now: Date, calendar: Calendar = .current) -> Bool {
        guard isEnabled else { return false }
        guard let last = lastFiredAt else {
            // Never fired: due once its first slot after creation has passed.
            guard let first = schedule.nextFireDate(after: createdAt, calendar: calendar)
            else { return false }
            return first <= now
        }
        guard let next = schedule.nextFireDate(after: last, calendar: calendar) else { return false }
        return next <= now
    }
}

/// The three recurrence shapes. Not a cron string: these cover every routine
/// a person asks for, and `everyMinutes` makes a routine demonstrable inside a
/// test run.
public enum RoutineSchedule: Codable, Hashable, Sendable {
    case everyMinutes(Int)
    case dailyAt(hour: Int, minute: Int)
    case weeklyOn(weekday: Int, hour: Int, minute: Int)   // weekday: 1 = Sunday

    public func nextFireDate(after reference: Date, calendar: Calendar = .current) -> Date? {
        switch self {
        case .everyMinutes(let m):
            guard m > 0 else { return nil }
            return reference.addingTimeInterval(TimeInterval(m) * 60)

        case .dailyAt(let h, let min):
            var c = DateComponents()
            c.hour = h
            c.minute = min
            c.second = 0
            return calendar.nextDate(after: reference, matching: c,
                                     matchingPolicy: .nextTime)

        case .weeklyOn(let wd, let h, let min):
            var c = DateComponents()
            c.weekday = wd
            c.hour = h
            c.minute = min
            c.second = 0
            return calendar.nextDate(after: reference, matching: c,
                                     matchingPolicy: .nextTime)
        }
    }

    /// The one-line description shown under a routine's title.
    public var label: String {
        switch self {
        case .everyMinutes(let m):
            if m == 1 { return "Every minute" }
            if m % 60 == 0 {
                let h = m / 60
                return h == 1 ? "Every hour" : "Every \(h) hours"
            }
            return "Every \(m) minutes"
        case .dailyAt(let h, let m):
            return "Every day at \(Self.clock(h, m))"
        case .weeklyOn(let wd, let h, let m):
            return "Every \(Self.weekdayName(wd)) at \(Self.clock(h, m))"
        }
    }

    public static func clock(_ hour: Int, _ minute: Int) -> String {
        let suffix = hour < 12 ? "AM" : "PM"
        var h = hour % 12
        if h == 0 { h = 12 }
        return String(format: "%d:%02d %@", h, minute, suffix)
    }

    public static func weekdayName(_ weekday: Int) -> String {
        let names = ["Sunday", "Monday", "Tuesday", "Wednesday",
                     "Thursday", "Friday", "Saturday"]
        let i = (weekday - 1) % 7
        return names[i < 0 ? 0 : i]
    }

    // MARK: Codable
    //
    // A flat tagged object (`{"kind": "dailyAt", "hour": 8, "minute": 0}`),
    // not the synthesised nested form: the file is read back by later builds
    // and by the TypeScript and Rust SDKs.

    private enum CodingKeys: String, CodingKey { case kind, minutes, hour, minute, weekday }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .everyMinutes(let m):
            try c.encode("everyMinutes", forKey: .kind)
            try c.encode(m, forKey: .minutes)
        case .dailyAt(let h, let min):
            try c.encode("dailyAt", forKey: .kind)
            try c.encode(h, forKey: .hour)
            try c.encode(min, forKey: .minute)
        case .weeklyOn(let wd, let h, let min):
            try c.encode("weeklyOn", forKey: .kind)
            try c.encode(wd, forKey: .weekday)
            try c.encode(h, forKey: .hour)
            try c.encode(min, forKey: .minute)
        }
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        switch try c.decode(String.self, forKey: .kind) {
        case "everyMinutes":
            self = .everyMinutes(try c.decode(Int.self, forKey: .minutes))
        case "dailyAt":
            self = .dailyAt(hour: try c.decode(Int.self, forKey: .hour),
                            minute: try c.decode(Int.self, forKey: .minute))
        case "weeklyOn":
            self = .weeklyOn(weekday: try c.decode(Int.self, forKey: .weekday),
                             hour: try c.decode(Int.self, forKey: .hour),
                             minute: try c.decode(Int.self, forKey: .minute))
        case let other:
            throw DecodingError.dataCorruptedError(
                forKey: .kind, in: c, debugDescription: "unknown schedule kind: \(other)")
        }
    }
}

/// What happened when the scheduler fired a routine.
///
/// A refusal is its own case, not a failure: a Bot mid-turn refusing another
/// turn is correct behaviour, and a routine that quietly swallowed it would
/// leave the user believing work happened.
public enum RoutineFiring: Equatable, Sendable {
    case started(runID: String)
    case refused(reason: String)
    case failed(reason: String)

    public var summary: String {
        switch self {
        case .started(let r):  return "started run \(r)"
        case .refused(let r):  return "refused: \(r)"
        case .failed(let r):   return "failed: \(r)"
        }
    }

    public var runID: String? { if case .started(let r) = self { return r }; return nil }
}

/// The seam between the scheduler and whatever actually runs a Bot.
///
/// The scheduler is pure clock logic and has no idea what a Space is; an
/// app's live runner turns a firing into a real agent turn.
@MainActor
public protocol RoutineRunner: AnyObject {
    func fire(_ routine: Routine) async -> RoutineFiring
}

/// Routines: storage, editing, and the clock that fires them.
///
/// Three things are kept deliberately separate:
///
/// 1. **Persistence**: a routine that does not survive an app restart is a
///    timer, not a routine, so the list is written to disk on every mutation.
/// 2. **The clock**: `tick(now:)` takes the instant to evaluate against. The
///    background loop is a thin wrapper over it, which is what makes the
///    scheduler testable without sleeping.
/// 3. **The firing**: delegated to a `RoutineRunner`, so the store itself has
///    no idea what a Space is.
@MainActor
public final class RoutineStore: ObservableObject {

    @Published public private(set) var routines: [Routine] = []
    /// The last thing the scheduler did, newest first. Shown in the panel so a
    /// firing, including a refused one, is visible rather than silent.
    @Published public private(set) var log: [FiringRecord] = []

    public struct FiringRecord: Identifiable, Equatable, Sendable {
        public let id = UUID()
        public var routineID: String
        public var title: String
        public var at: Date
        public var firing: RoutineFiring

        public init(routineID: String, title: String, at: Date, firing: RoutineFiring) {
            self.routineID = routineID
            self.title = title
            self.at = at
            self.firing = firing
        }
    }

    /// Where the list lives. Tests get their own file.
    public let fileURL: URL
    /// Held strongly: the runner owns the app's Bot store, so there is no
    /// cycle, and a deallocated runner would turn every firing into a silent
    /// no-op.
    private var runner: RoutineRunner?
    private var tickTask: Task<Void, Never>?

    public init(fileURL: URL, runner: RoutineRunner? = nil) {
        self.fileURL = fileURL
        self.runner = runner
        load()
    }

    deinit { tickTask?.cancel() }

    public func attach(runner: RoutineRunner) { self.runner = runner }

    // MARK: - Persistence

    /// Read the list back. A corrupt file is reported, not silently replaced
    /// with an empty list.
    public func load() {
        guard let data = try? Data(contentsOf: fileURL) else { routines = []; return }
        do {
            let decoder = JSONDecoder()
            decoder.dateDecodingStrategy = .custom(Self.decodeDate)
            routines = try decoder.decode([Routine].self, from: data)
        } catch {
            routines = []
            note("routines file could not be read (\(error)); starting empty")
        }
    }

    /// ISO 8601, with or without fractional seconds (the TypeScript SDK's
    /// `toISOString` writes them; this one and the Rust SDK do not).
    nonisolated static func decodeDate(_ decoder: Decoder) throws -> Date {
        let s = try decoder.singleValueContainer().decode(String.self)
        let plain = ISO8601DateFormatter()
        if let d = plain.date(from: s) { return d }
        let frac = ISO8601DateFormatter()
        frac.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let d = frac.date(from: s) { return d }
        throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath,
                                                debugDescription: "not an ISO 8601 date: \(s)"))
    }

    @discardableResult
    public func save() -> Bool {
        do {
            let encoder = JSONEncoder()
            encoder.dateEncodingStrategy = .iso8601
            encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
            try FileManager.default.createDirectory(at: fileURL.deletingLastPathComponent(),
                                                    withIntermediateDirectories: true)
            try encoder.encode(routines).write(to: fileURL, options: .atomic)
            return true
        } catch {
            note("could not save routines: \(error)")
            return false
        }
    }

    // MARK: - CRUD

    public func routines(for botID: String) -> [Routine] {
        routines.filter { $0.botID == botID }
            .sorted { $0.createdAt < $1.createdAt }
    }

    public func routine(_ id: String) -> Routine? { routines.first { $0.id == id } }

    @discardableResult
    public func create(botID: String, title: String, prompt: String,
                       schedule: RoutineSchedule, enabled: Bool = true,
                       now: Date = Date()) -> Routine {
        // Whole seconds: the saved form has no fractional part, so a reloaded
        // routine compares equal to the one in memory.
        let created = Date(timeIntervalSince1970: floor(now.timeIntervalSince1970))
        let r = Routine(botID: botID, title: title, prompt: prompt,
                        schedule: schedule, isEnabled: enabled, createdAt: created)
        routines.append(r)
        save()
        return r
    }

    public func update(_ routine: Routine) {
        guard let i = routines.firstIndex(where: { $0.id == routine.id }) else { return }
        routines[i] = routine
        save()
    }

    public func delete(_ id: String) {
        routines.removeAll { $0.id == id }
        save()
    }

    /// Enable or disable without deleting. A disabled routine keeps its firing
    /// history, so re-enabling does not make it look brand new.
    public func setEnabled(_ enabled: Bool, for id: String) {
        guard let i = routines.firstIndex(where: { $0.id == id }) else { return }
        routines[i].isEnabled = enabled
        save()
    }

    // MARK: - The clock

    /// Every routine that should fire at `now`.
    public func due(at now: Date, calendar: Calendar = .current) -> [Routine] {
        routines.filter { $0.isDue(at: now, calendar: calendar) }
    }

    /// Evaluate the clock once and fire whatever is due. Returns what it fired.
    @discardableResult
    public func tick(now: Date = Date(), calendar: Calendar = .current) async -> [FiringRecord] {
        var fired: [FiringRecord] = []
        for routine in due(at: now, calendar: calendar) {
            let record = await fire(routine, now: now)
            fired.append(record)
        }
        return fired
    }

    /// Fire one routine immediately, whatever the clock says. This is both the
    /// scheduler's inner step and the panel's "Run now" action.
    @discardableResult
    public func fire(_ routine: Routine, now: Date = Date()) async -> FiringRecord {
        let firing: RoutineFiring
        if let runner {
            firing = await runner.fire(routine)
        } else {
            firing = .failed(reason: "no runner attached: not connected to a Space")
        }
        // `lastFiredAt` advances even on a refusal, on purpose: the slot was
        // used. Leaving it unset would make the routine due forever and hammer
        // a busy Bot once per tick.
        let at = Date(timeIntervalSince1970: floor(now.timeIntervalSince1970))
        if let i = routines.firstIndex(where: { $0.id == routine.id }) {
            routines[i].lastFiredAt = at
            routines[i].lastRunID = firing.runID ?? routines[i].lastRunID
            routines[i].lastOutcome = firing.summary
            save()
        }
        let record = FiringRecord(routineID: routine.id, title: routine.title,
                                  at: at, firing: firing)
        log.insert(record, at: 0)
        if log.count > 50 { log.removeLast(log.count - 50) }
        return record
    }

    /// The background loop. One loop for every routine, not one timer each.
    public func startScheduler(every interval: Duration = .seconds(15)) {
        tickTask?.cancel()
        tickTask = Task { [weak self] in
            while !Task.isCancelled {
                guard let self else { return }
                await self.tick()
                try? await Task.sleep(for: interval)
            }
        }
    }

    public func stopScheduler() { tickTask?.cancel(); tickTask = nil }

    public var isSchedulerRunning: Bool { tickTask != nil && !(tickTask?.isCancelled ?? true) }

    private func note(_ text: String) {
        log.insert(FiringRecord(routineID: "", title: "Routines", at: Date(),
                                firing: .failed(reason: text)), at: 0)
    }
}
