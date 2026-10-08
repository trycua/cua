// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Agents: persistent agents, their runs in Spaces, a run's events, and the
 * coding agents on this machine. Hand-written mirrors, each naming its
 * source:
 *
 * - `PersistentAgent`: `cua-spaces-app-core/src/persistent.rs`
 *   `PersistentAgentInput` (what `persistent_agent_list` reports, camelCase);
 * - `SpaceAgentRun`: `cua-spaces-app-core/src/agents.rs` `SpaceAgentRun`
 *   (the Tauri `list_space_agents` row);
 * - `AgentEvent`, `AgentEventsPage`: `cua-agents/src/events.rs` `AgentEvent`
 *   and the `agent_events` tool's result (snake_case, as the tool answers);
 * - `AgentSetupRow`: `cua-spaces-app-core/src/agents.rs` `AgentSettingsRow`
 *   (the Tauri `agent_setup_detect` row).
 */

/** cua-agents' `RunStatus`, verbatim. `unknown` is never a guess. */
export type AgentRunStatus = "running" | "idle" | "failed" | "crashed" | "unknown";

export interface PersistentAgent {
  name: string;
  /** Harness id (`claude-code`, `openai-codex`, `hermes`, `openclaw`, ...). */
  harness: string;
  /** The Space it works in. */
  space: string;
  paused: boolean;
  /** `running`, `suspended` or `released`. */
  spaceState: string;
  /** Its current run, if any. */
  runId?: string | null;
  /** Unix ms of the last home save (0: never). */
  savedMs: number;
  lastError?: string | null;
}

export interface SpaceAgentRun {
  runId: string;
  /** Harness id. */
  agent: string;
  status: AgentRunStatus;
  /** Why it is in that status. */
  reason: string;
  /** The run's label or prompt, on one line. */
  summary: string;
  /** Unix seconds. */
  createdAt: number | null;
  /** Finer phase (`thinking`, `tool`, `waiting`, ...). */
  phase: string;
  turn: number;
}

/** How a conversation view shows an event (`cua_agents::events::CATEGORIES`). */
export type AgentEventCategory = "message" | "user" | "activity" | "hidden";

/** One normalized event of a run. The same kinds for every harness. */
export interface AgentEvent {
  /** Monotonic within a run. */
  seq: number;
  ts_ms: number;
  /** 0 before the first prompt. */
  turn: number;
  /** `turn_started`, `message`, `thought`, `tool_call`, `tool_update`, `plan`, `turn_ended`, `error`, ... */
  kind: string;
  /** Message, thought or prompt text (a message arrives in chunks). */
  text?: string;
  tool_id?: string;
  tool_title?: string;
  /** ACP tool kind (`execute`, `edit`, `read`, ...). */
  tool_kind?: string;
  /** ACP tool status (`pending`, `in_progress`, `completed`, `failed`). */
  tool_status?: string;
  stop_reason?: string;
  category: AgentEventCategory;
  /** One line for an `activity` event. */
  summary?: string;
}

/** `agent_events`: the events after a cursor. */
export interface AgentEventsPage {
  run_id: string;
  status: AgentRunStatus;
  phase: string;
  events: AgentEvent[];
  /** Pass back to continue. */
  cursor: number;
  /** Nothing more is written yet. */
  caught_up: boolean;
}

/** A coding agent on this machine, as the SDK's agent setup sees it. */
export interface AgentSetupRow {
  /** Setup id (`claude-code`, `codex`, `hermes`, `openclaw`, ...). */
  agent: string;
  name: string;
  installed: boolean;
  /** The cua skills and the cua MCP server are both in place (where supported). */
  configured: boolean;
  detail: string;
  skillsInstalled: number;
  skillsTotal: number;
  /** The MCP config file cua edits, when the agent has one. */
  mcpConfig: string | null;
  skillsDir: string | null;
}
