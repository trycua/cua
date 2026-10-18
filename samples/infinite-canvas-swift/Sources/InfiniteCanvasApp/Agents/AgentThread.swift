// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CanvasModel
import Cua
import Foundation

/// One line of a thread's transcript.
struct ThreadMessage: Identifiable, Equatable {
    enum Role: Equatable { case user, agent, tool, status }
    let id: Int
    var role: Role
    var text: String
}

/// Where a thread's agent runs and how it is launched.
struct AgentLaunch: Equatable {
    var harness: String
    /// Custom model endpoint (the scripted mock provider in the demo), and
    /// the key variable the harness reads for it.
    var baseURL: String?
    var model: String?
    var env: [String: String]
}

/// A conversation with one coding agent running over ACP in a Space, through
/// the cua SDK's `Agents` (`SpacesdClient.agents()`): `run` starts it,
/// `send` follows up in the same session, `events` pages the normalized ACP
/// events (messages, tool calls, turn boundaries).
@MainActor
final class AgentThread: ObservableObject, Identifiable {
    let id: String
    let spaceID: String
    let spaceLabel: String
    @Published private(set) var messages: [ThreadMessage] = []
    @Published private(set) var isTurnRunning = false
    @Published private(set) var status: String = ""
    @Published var draft: String = ""

    private let agents: () async throws -> Agents
    private let launch: AgentLaunch
    private var run: AgentRun?
    private var cursor: UInt64 = 0
    private var poll: Task<Void, Never>?
    private var nextID = 0
    /// Streaming message chunks join the agent line of the current turn.
    private var openAgentLine: Int?
    /// Called on every tool call with its title, so the canvas can play the
    /// matching cursor action.
    var onToolCall: ((String) -> Void)?
    var onTurnChange: ((Bool) -> Void)?

    init(id: String, spaceID: String, spaceLabel: String, launch: AgentLaunch,
         agents: @escaping () async throws -> Agents) {
        self.id = id
        self.spaceID = spaceID
        self.spaceLabel = spaceLabel
        self.launch = launch
        self.agents = agents
    }

    var title: String { "Agent · \(spaceLabel)" }

    private func append(_ role: ThreadMessage.Role, _ text: String) {
        messages.append(ThreadMessage(id: nextID, role: role, text: text))
        nextID += 1
    }

    /// Send the draft (or `text`): the first message starts the run, later
    /// ones follow up in the same ACP session.
    func send(_ text: String? = nil) {
        let body = (text ?? draft).trimmingCharacters(in: .whitespacesAndNewlines)
        guard !body.isEmpty else { return }
        draft = ""
        append(.user, body)
        openAgentLine = nil
        setTurn(true)
        Task {
            do {
                if let run {
                    _ = try await run.send(text: body, files: nil)
                } else {
                    status = "Starting \(launch.harness)"
                    let options = AgentRunOptions(env: launch.env, model: launch.model, baseUrl: launch.baseURL,
                                                  label: "infinite-canvas")
                    run = try await agents().run(harness: launch.harness, prompt: body, options: options)
                    status = ""
                }
                startPolling()
            } catch {
                append(.status, "Could not send: \(error)")
                setTurn(false)
            }
        }
    }

    func setTurn(_ on: Bool) {
        guard isTurnRunning != on else { return }
        isTurnRunning = on
        onTurnChange?(on)
    }

    private func startPolling() {
        guard poll == nil else { return }
        poll = Task { [weak self] in
            var idle = 0
            // Bounded: stops after ~10 minutes without a running turn.
            while !Task.isCancelled, idle < 1_500 {
                guard let self else { return }
                let running = await self.pollOnce()
                idle = running ? 0 : idle + 1
                try? await Task.sleep(for: .milliseconds(running ? 250 : 400))
            }
            self?.poll = nil
        }
    }

    /// One page of events. Returns whether a turn is still running.
    private func pollOnce() async -> Bool {
        guard let run else { return false }
        do {
            let page = try await run.events(cursor: cursor, max: 200)
            cursor = page.cursor
            for e in page.events { apply(e) }
            return isTurnRunning
        } catch {
            status = "\(error)"
            return isTurnRunning
        }
    }

    private func apply(_ e: AgentEvent) {
        // Install progress is only news until the agent starts working.
        if ["message", "tool_call", "turn_started", "turn_ended"].contains(e.kind) { status = "" }
        switch e.kind {
        case "message":
            guard let t = e.text, !t.isEmpty else { return }
            if let i = openAgentLine, let idx = messages.firstIndex(where: { $0.id == i }) {
                messages[idx].text += t
            } else {
                append(.agent, t)
                openAgentLine = messages.last?.id
            }
        case "tool_call":
            openAgentLine = nil
            let title = e.toolTitle ?? e.line ?? "tool"
            append(.tool, Self.toolLine(title))
            onToolCall?(title)
        case "turn_started":
            setTurn(true)
        case "turn_ended", "exited":
            openAgentLine = nil
            setTurn(false)
        case "error":
            append(.status, e.line ?? e.text ?? "error")
        case "install":
            status = "Installing"
        default:
            break
        }
    }

    /// `mcp__cua-driver__click` -> `click`.
    static func toolLine(_ title: String) -> String {
        let base = title.split(separator: "__").last.map(String.init) ?? title
        return base.replacingOccurrences(of: "_", with: " ")
    }

    func stop() async {
        poll?.cancel()
        poll = nil
        if let run { _ = try? await run.stop() }
    }
}
