// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation

/// What happened in a bot's run since the last poll.
public enum EngineUpdate: Equatable, Sendable {
    /// The assistant's text for `turn` so far (the whole turn, not a delta).
    case assistant(turn: UInt32, text: String)
    /// A tool the harness is running ("Run tests", "Open browser").
    case tool(turn: UInt32, title: String)
    case turnEnded(turn: UInt32)
    case failed(String)
}

/// Where a bot is in its life, as the engine reports it.
public enum EnginePhase: Equatable, Sendable {
    case creatingSpace(String)
    case startingAgent
    case ready
}

/// The seam between the app and Cua: a Space per bot, an agent run in it, a
/// Volume home copied in and out. `CuaBotsCua.CuaEngine` is the real one; tests
/// use a scripted engine.
@MainActor
public protocol BotEngine: AnyObject {
    /// Create (or reuse) the bot's own Space. Returns its id.
    func provision(_ bot: Bot, progress: @escaping @MainActor (EnginePhase) -> Void) async throws -> String

    /// Copy the bot's home from the Volume into its Space and start its agent
    /// with `prompt` as the first turn. Returns the run id.
    func start(_ bot: Bot, volume: VolumeStore, prompt: String) async throws -> String

    /// Copy what changed on the app's side (instructions, rules, the inbox)
    /// from the Volume into the Space, before a turn.
    func push(_ bot: Bot, volume: VolumeStore) async throws

    /// Send a message to the running agent (starting a new turn).
    func send(_ bot: Bot, text: String) async throws

    /// Updates since `cursor`, and the new cursor.
    func poll(_ bot: Bot, cursor: UInt64) async throws -> (updates: [EngineUpdate], cursor: UInt64)

    /// Copy the bot's home (memory, outputs) from its Space back into the Volume.
    func checkpoint(_ bot: Bot, volume: VolumeStore) async throws

    /// Stop the run's work (the run and Space stay; the next message resumes
    /// the session).
    func interrupt(_ bot: Bot) async throws

    /// Pause: stop work and suspend the Space where the runtime supports it.
    func pause(_ bot: Bot) async throws
    func resume(_ bot: Bot) async throws

    /// Delete the run and the Space.
    func reset(_ bot: Bot) async throws
}
