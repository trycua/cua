// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
import Foundation
import Observation

/// A Space's coding-agent runs for its detail. A failed read is its own
/// state, never an empty list: "no agents" and "could not ask" are
/// different claims.
@MainActor
@Observable
public final class AgentRunsModel {
    public enum Load: Equatable {
        case loading
        case ready([AppSpaceAgentRun])
        case failed(String)
    }

    public private(set) var load: Load = .loading
    let backend: SpacesBackend
    let spaceId: String

    public init(backend: SpacesBackend, spaceId: String) {
        self.backend = backend
        self.spaceId = spaceId
    }

    /// Re-reads the runs.
    public func refresh() async {
        do {
            load = .ready(try await backend.agentRuns(id: spaceId))
        } catch {
            load = .failed(LiveSpacesBackend.words(error))
        }
    }

    /// Reads every `seconds` until cancelled (the detail's lifetime).
    public func poll(every seconds: Double = 4) async {
        while !Task.isCancelled {
            await refresh()
            try? await Task.sleep(for: .seconds(seconds))
        }
    }

    /// One line per run: the agent, then what it was asked.
    public static func line(_ run: AppSpaceAgentRun) -> String {
        "\(appAgentName(agent: run.agent)) \u{b7} \(appAgentSubtitle(run: run))"
    }

    /// The status word.
    public static func status(_ run: AppSpaceAgentRun) -> String {
        appAgentStatusLabel(status: run.status)
    }
}
