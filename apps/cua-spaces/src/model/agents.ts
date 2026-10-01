// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { core } from "../core";

/**
 * Coding agents running inside a Space: the wire type and the pure helpers the
 * AGENTS section renders from.
 *
 * The status vocabulary is cua-agents' `RunStatus`, verbatim, and the shell
 * hands it over unchanged, so the SDK, the CLI, the MCP and this panel describe
 * the same run the same way.
 */

/**
 * What an agent run is doing. Every value comes from a real signal (the run's
 * state file and process liveness). `unknown` means the Space could not be
 * read and is never a stand-in for a guess.
 *
 * `idle` means no turn is running and a follow-up is accepted.
 */
export type AgentStatus = "running" | "idle" | "failed" | "crashed" | "unknown";

export interface SpaceAgentRun {
  runId: string;
  /** Harness id (`claude-code`, `openai-codex`, ...). Empty when unreadable. */
  agent: string;
  status: AgentStatus;
  /** Why it is that status, in words: the row's tooltip. */
  reason: string;
  /** One line summarising the prompt the agent was given. */
  summary: string;
  /** Unix seconds; null when the record could not be read. */
  createdAt: number | null;
  /** The SDK's finer phase: `installing`, `starting`, `working`, `waiting`, ... */
  phase: string;
  /** Turns the run has completed or started. */
  turn: number;
}

/** The label under each status, in the `STATUS_LABEL` idiom. */
export const AGENT_STATUS_LABEL: Record<AgentStatus, string> = {
  running: "Running",
  idle: "Idle",
  failed: "Failed",
  crashed: "Crashed",
  unknown: "Unknown",
};

/**
 * Which `status-*` dot each agent status wears: the existing Space dots doing
 * the same job (green for healthy, blue and blinking for in-progress) rather
 * than a second palette. Only `failed`/`crashed` needed a colour the Space
 * statuses never had.
 */
export const AGENT_STATUS_DOT: Record<AgentStatus, string> = {
  running: "status-provisioning",
  idle: "status-running",
  failed: "status-error",
  crashed: "status-error",
  unknown: "status-unknown",
};

/** Display name for a harness id. Unknown ids show their id, not a blank. */
export const AGENT_NAME: Record<string, string> = {
  "claude-code": "Claude Code",
  "openai-codex": "OpenAI Codex",
  "gemini-cli": "Gemini CLI",
  "google-antigravity": "Google Antigravity",
  opencode: "OpenCode",
  goose: "Goose",
  pi: "Pi",
  hermes: "Hermes",
  openclaw: "OpenClaw",
};

export function agentName(agent: string): string {
  return core("agents.name", { agent });
}

/** Monogram for a harness with no thumbnail of its own. */
export function agentInitial(agent: string): string {
  return core("agents.initial", { agent });
}

/**
 * The line under the agent's name. Falls back to naming the absence rather than
 * rendering an empty row — a run whose prompt could not be read is a fact worth
 * showing, not a blank.
 */
export function agentSubtitle(run: SpaceAgentRun): string {
  return core("agents.subtitle", { run });
}

/** Filter rows against the window's search box, over everything a row shows. */
export function filterAgentRuns(runs: SpaceAgentRun[], query: string): SpaceAgentRun[] {
  return core("agents.filter", { runs, query });
}

/**
 * Rows in the order they should be read: whatever needs a human first, then
 * newest within each status. The shell already sorts newest-first, and this is
 * a stable re-sort on top of that.
 */
export function orderAgentRuns(runs: SpaceAgentRun[]): SpaceAgentRun[] {
  return core("agents.order", { runs });
}
