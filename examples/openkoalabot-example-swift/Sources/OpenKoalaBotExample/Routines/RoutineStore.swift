// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpaces
import Foundation

/// The app's routines file. `RoutineStore` itself (persistence, the clock,
/// the firing log) is the SDK's (`CuaSpaces`); this app supplies where the
/// list lives and the live runner below.
extension RoutineStore {
    static func defaultFileURL() -> URL {
        AppDataDirectory.url().appendingPathComponent("routines.json")
    }

    /// The app's store, in the app data directory.
    convenience init() {
        self.init(fileURL: Self.defaultFileURL())
    }
}

// MARK: - The live runner

/// Turns a routine firing into a real agent run in the shared Space.
///
/// Firing is deliberately *not* "send text and hope":
///
/// - a Bot with no agent thread is **hired** — a real `agent_start`, which is
///   what makes a routine observable as a run in `agent_list`;
/// - a hired Bot that will take a message gets a new turn on its existing
///   thread, because a Bot is one persistent coworker with one transcript;
/// - a Bot mid-turn is **refused**, not interrupted. A routine must never
///   abandon work the user asked for.
@MainActor
final class BotStoreRoutineRunner: RoutineRunner {
    private let store: BotStore
    /// Marks a routine-originated turn in the transcript so the user can tell
    /// scheduled work from something they typed.
    static let prefix = Routine.prefix

    init(store: BotStore) { self.store = store }

    func fire(_ routine: Routine) async -> RoutineFiring {
        let presence = store.presence(for: routine.botID)
        let text = routine.turnText

        if presence.state == .running {
            return .refused(reason: "\(routine.botID) is mid-turn; the slot was skipped")
        }

        if presence.hasThread, presence.acceptsMessage {
            let outcome = await store.send(text, to: routine.botID)
            if outcome.accepted {
                return .started(runID: store.runID(for: routine.botID) ?? "")
            }
            return .refused(reason: outcome.reason)
        }

        do {
            let runID = try await store.hire(routine.botID, prompt: text)
            return .started(runID: runID)
        } catch {
            return .failed(reason: "\(error)")
        }
    }
}
