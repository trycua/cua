// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
import Foundation
import Observation

/// One run's conversation as it streams in: `agent_events` pages folded by
/// the SDK's `AgentTranscript` (cua-agents' `Transcript`, the rule the web
/// UI's `transcript.ts` mirrors), so both UIs show the same items and labels.
@MainActor
@Observable
public final class RunTimelineModel: Identifiable {
    public private(set) var items: [AgentTranscriptItem] = []
    /// The run's status from its latest page; the run row's until then.
    public private(set) var status: AppAgentStatus
    /// `responding`, `tool`, ... while a turn runs.
    public private(set) var phase: String
    public private(set) var loading = true
    /// The last read failed (items read before it stay).
    public private(set) var error: String?

    let backend: SpacesBackend
    let spaceId: String
    public let run: AppSpaceAgentRun
    private let transcript = AgentTranscript()
    private var cursor: UInt64 = 0

    /// Pages read per poll before waiting, as the web UI's store.
    static let maxPages = 20

    public init(backend: SpacesBackend, spaceId: String, run: AppSpaceAgentRun) {
        self.backend = backend
        self.spaceId = spaceId
        self.run = run
        self.status = run.status
        self.phase = run.phase
    }

    public var live: Bool { status == .running }

    /// Reads every page written since the last read.
    public func refresh() async {
        do {
            for _ in 0..<Self.maxPages {
                let json = try await backend.agentEvents(id: spaceId, runId: run.runId, cursor: cursor)
                if let c = try transcript.absorbJson(json: json) { cursor = max(cursor, c) }
                let page = (try? JSONSerialization.jsonObject(with: Data(json.utf8))) as? [String: Any] ?? [:]
                if let s = page["status"] as? String { status = Self.status(s) }
                if let p = page["phase"] as? String { phase = p }
                if page["caught_up"] as? Bool ?? true { break }
            }
            items = transcript.items()
            error = nil
        } catch {
            items = transcript.items()
            self.error = LiveSpacesBackend.words(error)
        }
        loading = false
    }

    /// Reads while the view is open: every half second while a turn runs,
    /// every 4 s otherwise (the web UI's intervals).
    public func poll() async {
        while !Task.isCancelled {
            await refresh()
            try? await Task.sleep(for: .milliseconds(live ? 500 : 4000))
        }
    }

    static func status(_ s: String) -> AppAgentStatus {
        switch s {
        case "running": .running
        case "idle": .idle
        case "failed": .failed
        case "crashed": .crashed
        default: .unknown
        }
    }

    /// The line under the last item: what the run is doing now (the web
    /// UI's `RunFooter`), or nil.
    public var footer: (text: String, failed: Bool)? {
        switch status {
        case .running:
            return (phase == "responding" ? "Writing" : phase == "tool" ? "Using tools" : "Working", false)
        case .failed, .crashed:
            let label = appAgentStatusLabel(status: status)
            return (run.reason.isEmpty ? label : "\(label): \(run.reason)", true)
        case .idle:
            return ("Waiting for a follow-up.", false)
        case .unknown:
            return nil
        }
    }

    /// The activity group that stays open while its turn runs: the newest
    /// one of the last item's turn.
    public var openActivity: Int? {
        guard live, let lastTurn = items.last?.turn else { return nil }
        return items.indices.last { items[$0].kind == "activity" && items[$0].turn == lastTurn }
    }
}
