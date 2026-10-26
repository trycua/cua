// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaBotsCore
import CuaSpaces
import Foundation

/// "Works on its own": a bot's scheduled tasks run on the SDK's routine clock
/// (`CuaSpaces.RoutineStore`). Each scheduled task is one routine whose prompt
/// is the task id; firing asks the store to run the task, and a paused or
/// busy bot refuses rather than interrupts.
@MainActor
public final class RoutineClock: RoutineRunner {
    public let routines: RoutineStore
    private weak var store: BotStore?

    public init(fileURL: URL, store: BotStore) {
        routines = RoutineStore(fileURL: fileURL)
        self.store = store
        routines.attach(runner: self)
        store.onTasksChanged = { [weak self] bot, tasks in self?.sync(bot: bot, tasks: tasks) }
        for bot in store.bots { sync(bot: bot, tasks: store.tasks(for: bot.id)) }
    }

    public func start(every interval: Duration = .seconds(15)) {
        routines.startScheduler(every: interval)
    }

    /// Mirror one bot's scheduled tasks into the routine list, keeping each
    /// routine's firing history.
    public func sync(bot: Bot, tasks: [BotTask]) {
        let scheduled = tasks.filter { $0.schedule != nil }
        let existing = routines.routines(for: bot.id)
        for r in existing where !scheduled.contains(where: { $0.id == r.prompt }) {
            routines.delete(r.id)
        }
        for task in scheduled {
            guard let schedule = task.schedule.map(Self.routineSchedule) else { continue }
            let enabled = task.state == .scheduled && !bot.isPaused
            if var r = existing.first(where: { $0.prompt == task.id }) {
                if r.schedule != schedule || r.isEnabled != enabled || r.title != task.title {
                    r.schedule = schedule
                    r.isEnabled = enabled
                    r.title = task.title
                    routines.update(r)
                }
            } else {
                routines.create(botID: bot.id, title: task.title, prompt: task.id, schedule: schedule,
                                enabled: enabled)
            }
        }
    }

    public func fire(_ routine: Routine) async -> RoutineFiring {
        guard let store else { return .failed(reason: "the app is closing") }
        guard let bot = store.bot(routine.botID) else { return .failed(reason: "no such bot") }
        if bot.isPaused { return .refused(reason: "\(bot.name) is paused") }
        if store.isBusy(bot.id) { return .refused(reason: "\(bot.name) is mid-turn") }
        let ok = await store.fire(taskID: routine.prompt, botID: routine.botID)
        return ok ? .started(runID: store.bot(bot.id)?.runID ?? "") : .refused(reason: "not ready")
    }

    static func routineSchedule(_ s: TaskSchedule) -> RoutineSchedule {
        switch s {
        case .every(let m): .everyMinutes(m)
        case .daily(let h, let m): .dailyAt(hour: h, minute: m)
        case .weekly(let wd, let h, let m): .weeklyOn(weekday: wd, hour: h, minute: m)
        }
    }
}
