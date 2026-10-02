// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation

// MARK: - Conversation

public enum MessageRole: String, Codable, Sendable { case user, bot, system }

/// One entry in a bot's conversation.
public struct ChatMessage: Identifiable, Codable, Hashable, Sendable {
    public enum Kind: Codable, Hashable, Sendable {
        case text
        /// An approval card for `ApprovalRequest.id`.
        case approval(id: String)
        /// "Log me into this site": the secure sign-in card.
        case login(site: String, approvalID: String)
        /// A finished piece of work, with Volume paths to its outputs.
        case result(title: String, outputs: [String])
        /// A routine firing or other quiet notice.
        case notice
    }

    public var id: String
    public var botID: String
    public var role: MessageRole
    public var text: String
    public var kind: Kind
    public var date: Date
    public var readAt: Date?

    public init(id: String = UUID().uuidString, botID: String, role: MessageRole, text: String,
                kind: Kind = .text, date: Date = Date(), readAt: Date? = nil) {
        self.id = id
        self.botID = botID
        self.role = role
        self.text = text
        self.kind = kind
        self.date = date
        self.readAt = readAt
    }
}

// MARK: - Work

/// When a task repeats. The same three shapes as the SDK's `RoutineSchedule`
/// (`CuaSpaces`), which runs the clock on macOS.
public enum TaskSchedule: Codable, Hashable, Sendable {
    case every(minutes: Int)
    case daily(hour: Int, minute: Int)
    /// weekday: 1 is Sunday, as `Calendar` counts.
    case weekly(weekday: Int, hour: Int, minute: Int)

    public var label: String {
        switch self {
        case .every(let m): m % 60 == 0 ? "Every \(m / 60) h" : "Every \(m) min"
        case .daily: "Daily"
        case .weekly(let wd, _, _): "Weekly on \(Calendar.current.weekdaySymbols[(wd - 1 + 7) % 7])"
        }
    }

    public func nextFire(after date: Date, calendar: Calendar = .current) -> Date? {
        switch self {
        case .every(let minutes):
            return date.addingTimeInterval(TimeInterval(max(minutes, 1) * 60))
        case .daily(let h, let m):
            return calendar.nextDate(after: date, matching: DateComponents(hour: h, minute: m),
                                     matchingPolicy: .nextTime)
        case .weekly(let wd, let h, let m):
            return calendar.nextDate(after: date, matching: DateComponents(hour: h, minute: m, weekday: wd),
                                     matchingPolicy: .nextTime)
        }
    }
}

public enum TaskState: String, Codable, CaseIterable, Sendable {
    case inProgress, scheduled, paused, completed, failed

    public var title: String {
        switch self {
        case .inProgress: "In progress"
        case .scheduled: "Scheduled"
        case .paused: "Paused"
        case .completed: "Completed"
        case .failed: "Needs a look"
        }
    }
}

/// A step the bot reported while working.
public struct TaskStep: Identifiable, Codable, Hashable, Sendable {
    public var id: String
    public var title: String
    public var done: Bool
    public init(id: String = UUID().uuidString, title: String, done: Bool = false) {
        self.id = id
        self.title = title
        self.done = done
    }
}

/// Something the bot is doing or will do: a one-off task, a monitor, or a
/// routine on a schedule. Scheduled tasks live in the Volume
/// (`agents/<name>/tasks.json`) and fire from the routine clock.
public struct BotTask: Identifiable, Codable, Hashable, Sendable {
    public var id: String
    public var botID: String
    public var title: String
    public var prompt: String
    public var symbol: String
    public var state: TaskState
    public var schedule: TaskSchedule?
    public var nextRun: Date?
    public var lastRun: Date?
    public var notifyOnCompletion: Bool
    public var steps: [TaskStep]
    /// Volume paths under `agents/<name>/outputs/`.
    public var outputs: [String]
    public var createdAt: Date

    public init(id: String = UUID().uuidString, botID: String, title: String, prompt: String,
                symbol: String = "circle.dashed", state: TaskState = .inProgress,
                schedule: TaskSchedule? = nil, notifyOnCompletion: Bool = true,
                createdAt: Date = Date()) {
        self.id = id
        self.botID = botID
        self.title = title
        self.prompt = prompt
        self.symbol = symbol
        self.state = state
        self.schedule = schedule
        self.notifyOnCompletion = notifyOnCompletion
        self.steps = []
        self.outputs = []
        self.createdAt = createdAt
        self.nextRun = schedule?.nextFire(after: createdAt)
    }

    /// "Today 2:00 PM · Monitoring", "Tomorrow 9:00 AM · Daily".
    public func subtitle(now: Date = Date(), calendar: Calendar = .current) -> String {
        var parts: [String] = []
        if let next = nextRun { parts.append(Self.relative(next, now: now, calendar: calendar)) }
        if let s = schedule { parts.append(s.label) }
        else if state == .inProgress { parts.append(steps.isEmpty ? "Working" : "\(steps.filter(\.done).count) of \(steps.count) steps") }
        else if state == .completed, let last = lastRun { parts.append(Self.relative(last, now: now, calendar: calendar)) }
        return parts.joined(separator: " · ")
    }

    static func relative(_ date: Date, now: Date, calendar: Calendar) -> String {
        let time = date.formatted(date: .omitted, time: .shortened)
        if calendar.isDate(date, inSameDayAs: now) { return "Today \(time)" }
        if let tomorrow = calendar.date(byAdding: .day, value: 1, to: now),
           calendar.isDate(date, inSameDayAs: tomorrow) { return "Tomorrow \(time)" }
        if let yesterday = calendar.date(byAdding: .day, value: -1, to: now),
           calendar.isDate(date, inSameDayAs: yesterday) { return "Yesterday \(time)" }
        return date.formatted(.dateTime.month(.abbreviated).day().hour().minute())
    }
}

// MARK: - Approvals

/// Where an approval came from.
public enum ApprovalSource: Codable, Hashable, Sendable {
    /// The bot asked, following a custom rule.
    case rule(ruleID: String?)
    /// The bot asked the user to take over (passwords, money).
    case handOff
    /// A Keyvault access request (a saved sign-in for a site).
    case keyvault(requestID: String)
    /// "Log me into this site".
    case login(site: String)
}

public enum ApprovalState: String, Codable, Sendable { case pending, approved, denied, handedOff }

public struct ApprovalRequest: Identifiable, Codable, Hashable, Sendable {
    public var id: String
    public var botID: String
    public var action: String
    public var detail: String
    public var source: ApprovalSource
    public var state: ApprovalState
    public var createdAt: Date
    public var decidedAt: Date?

    public init(id: String = UUID().uuidString, botID: String, action: String, detail: String = "",
                source: ApprovalSource = .rule(ruleID: nil), state: ApprovalState = .pending,
                createdAt: Date = Date()) {
        self.id = id
        self.botID = botID
        self.action = action
        self.detail = detail
        self.source = source
        self.state = state
        self.createdAt = createdAt
    }
}

// MARK: - Notifications

public struct BotNotification: Identifiable, Codable, Hashable, Sendable {
    public enum Kind: String, Codable, Sendable { case result, approval, update, problem }

    public var id: String
    public var botID: String
    public var kind: Kind
    public var title: String
    public var body: String
    public var date: Date
    public var read: Bool

    public init(id: String = UUID().uuidString, botID: String, kind: Kind, title: String,
                body: String, date: Date = Date(), read: Bool = false) {
        self.id = id
        self.botID = botID
        self.kind = kind
        self.title = title
        self.body = body
        self.date = date
        self.read = read
    }
}
