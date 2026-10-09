// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { AgentRunStatus, AgentRunSummary, AgentState, AgentsData, AgentSummary, RunRef } from "@/bridge";

export const AGENT_STATE_LABEL: Record<AgentState, string> = { running: "Running", paused: "Paused", idle: "Idle" };

export const RUN_STATUS_LABEL: Record<AgentRunStatus, string> = {
  running: "Running",
  idle: "Idle",
  failed: "Failed",
  crashed: "Crashed",
  unknown: "Unknown",
};

/** The dot next to a state: green while working, red when a run needs a look. */
export type Tone = "live" | "quiet" | "paused" | "problem";

export const agentTone = (s: AgentState): Tone => (s === "running" ? "live" : s === "paused" ? "paused" : "quiet");
export const runTone = (s: AgentRunStatus): Tone =>
  s === "running" ? "live" : s === "failed" || s === "crashed" ? "problem" : s === "unknown" ? "paused" : "quiet";

/** `now`, `4m`, `3h`, `2d`: the compact age a list row shows. */
export function shortAge(then: number | null, now: number): string {
  if (!then) return "";
  const s = Math.max(0, Math.round((now - then) / 1000));
  if (s < 60) return "now";
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86_400) return `${Math.floor(s / 3600)}h`;
  return `${Math.floor(s / 86_400)}d`;
}

/** The Space's short name: `relay:linux-box/ubuntu-build` reads as `ubuntu-build`. */
export function spaceLabel(name: string): string {
  const tail = name.includes("/") ? name.slice(name.lastIndexOf("/") + 1) : name.replace(/^[a-z]+:/, "");
  return tail || name;
}

export const monogram = (harnessName: string) => harnessName.trim().charAt(0).toUpperCase() || "?";

/** What the URL selects: a persistent agent by name, or a run by Space and id. */
export interface AgentsSelection {
  agent?: string;
  space?: string;
  run?: string;
}

export interface Selected {
  agent: AgentSummary | null;
  run: AgentRunSummary | null;
  /** The run whose timeline the detail shows. */
  ref: RunRef | null;
}

/**
 * The row the URL names, else the first that needs a look: a running agent,
 * then the first run. A persistent agent opens its current run, or its
 * latest one.
 */
export function resolveSelection(data: AgentsData | undefined, sel: AgentsSelection): Selected {
  const none: Selected = { agent: null, run: null, ref: null };
  if (!data) return none;
  const findRun = (ref: RunRef | null) => (ref ? (data.runs.find((r) => r.spaceId === ref.spaceId && r.runId === ref.runId) ?? null) : null);
  const forAgent = (agent: AgentSummary): Selected => ({ agent, run: findRun(agent.run), ref: agent.run });
  if (sel.agent) {
    const agent = data.agents.find((a) => a.name === sel.agent);
    if (agent) return forAgent(agent);
  }
  if (sel.run && sel.space) {
    const run = data.runs.find((r) => r.spaceId === sel.space && r.runId === sel.run);
    if (run) {
      const agent = run.agentName ? (data.agents.find((a) => a.name === run.agentName) ?? null) : null;
      return { agent, run, ref: { spaceId: run.spaceId, runId: run.runId } };
    }
  }
  const running = data.agents.find((a) => a.state === "running");
  if (running) return forAgent(running);
  if (data.agents[0]) return forAgent(data.agents[0]);
  const run = data.runs[0];
  return run ? { agent: null, run, ref: { spaceId: run.spaceId, runId: run.runId } } : none;
}
