// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpacesFFI
import Foundation
import Observation

/// The getting-started checklist: the core's `appChecklistReduce` and
/// `appChecklistView`, kept in `checklist.json` beside the settings file.
/// The one copy: the New UI reads and changes it through the bridge
/// (`checklist.get`, `checklist.send`), so both UIs count the same.
@MainActor @Observable
public final class ChecklistModel {
    public private(set) var state: AppChecklistState
    private let path: String?
    /// Where usage events go (`checklist_<step>`, `checklist_completed`, ...).
    var telemetry: TelemetryRunning?

    /// `path`: the file it is kept in; nil keeps it in memory (tests).
    public init(path: String?) {
        self.path = path
        state = path.flatMap(Self.read) ?? AppChecklistState(done: [], dismissed: false, shown: false)
    }

    public var view: AppChecklistView { appChecklistView(state: state) }

    public func send(_ action: AppChecklistAction) {
        let t = appChecklistReduce(state: state, action: action)
        // A merge changes the state without events (they were recorded
        // where the steps were ticked).
        if t.state != state {
            state = t.state
            save()
        }
        if !t.signals.isEmpty { telemetry?.record(t.signals) }
    }

    public func done(_ step: AppChecklistStep) { send(.done(step: step)) }

    // MARK: - The file

    /// What the file holds: the core's state with each step as its word.
    private struct Stored: Codable {
        var done: [String]
        var dismissed: Bool
        var shown: Bool
    }

    static let words: [(AppChecklistStep, String)] = [
        (.addMachine, "add_machine"), (.createSpace, "create_space"), (.teleport, "teleport"),
        (.share, "share"), (.runAgent, "run_agent"),
    ]

    private static func read(_ path: String) -> AppChecklistState? {
        guard let data = FileManager.default.contents(atPath: path),
              let s = try? JSONDecoder().decode(Stored.self, from: data) else { return nil }
        return state(done: s.done, dismissed: s.dismissed, shown: s.shown)
    }

    /// The core's state from steps as words (unknown words are dropped).
    static func state(done: [String], dismissed: Bool, shown: Bool) -> AppChecklistState {
        AppChecklistState(done: words.filter { done.contains($0.1) }.map(\.0), dismissed: dismissed, shown: shown)
    }

    /// The state as the file and the web UI keep it (steps as words).
    var stored: [String: Any] {
        ["done": state.done.compactMap { step in Self.words.first { $0.0 == step }?.1 },
         "dismissed": state.dismissed, "shown": state.shown]
    }

    private func save() {
        guard let path else { return }
        let done = state.done.compactMap { step in Self.words.first { $0.0 == step }?.1 }
        guard let data = try? JSONEncoder().encode(Stored(done: done, dismissed: state.dismissed, shown: state.shown))
        else { return }
        let url = URL(fileURLWithPath: path)
        try? FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try? data.write(to: url, options: .atomic)
    }
}
