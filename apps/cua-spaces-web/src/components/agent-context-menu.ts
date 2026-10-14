// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The Agents page's context menus: a persistent agent (its row and its
 * detail's header) and a run (its row). The page's own actions, by the same
 * rules: Pause or Resume as the detail's button says, Stop while a run is
 * running, and Remove… for a persistent agent (its home stays in Cua
 * Volume).
 */

import { useNavigate } from "@tanstack/react-router";

import { useAgents, type AgentRunSummary, type AgentSummary } from "@/bridge";
import { SEPARATOR, useContextMenu, type ContextMenuEntry, type ContextMenuProps } from "@/components/context-menu";
import { toast, toastError } from "@/components/ui/toast";

/** What an agent's menu runs: the page's own actions. */
export interface AgentMenuActions {
  /** Selects it on the Agents page. */
  open(): void;
  pause(): Promise<unknown>;
  resume(): Promise<unknown>;
  /** Stops its current run. */
  stop(): Promise<unknown>;
  remove(): Promise<unknown>;
}

/** The run a persistent agent is running now, if any (what Stop stops). */
export function currentRun(agent: AgentSummary): { spaceId: string; runId: string } | null {
  return agent.state === "running" && agent.record.runId ? { spaceId: agent.spaceId, runId: agent.record.runId } : null;
}

/**
 * The entries of `agent`'s menu. Stable action ids: `agent.open`,
 * `agent.pause`, `agent.resume`, `agent.stop`, `agent.copyName` and
 * `agent.remove`. `open: false` leaves Open out (the agent's own detail).
 */
export function agentMenuEntries(agent: AgentSummary, act: AgentMenuActions, { open = true }: { open?: boolean } = {}): ContextMenuEntry[] {
  return [
    open && { id: "agent.open", label: "Open", run: act.open },
    SEPARATOR,
    // The detail's button, in the core's word: Resume while paused, Pause otherwise.
    agent.state === "paused"
      ? { id: "agent.resume", label: agent.actionLabel || "Resume", run: act.resume }
      : { id: "agent.pause", label: agent.actionLabel || "Pause", run: act.pause },
    currentRun(agent) && { id: "agent.stop", label: "Stop", run: act.stop },
    SEPARATOR,
    { id: "agent.copyName", label: "Copy Name", copy: agent.name },
    SEPARATOR,
    {
      id: "agent.remove",
      label: "Remove…",
      destructive: true,
      confirm: {
        title: `Remove ${agent.name}?`,
        message: "Its run stops and its routines go away. Its home and memory stay in Cua Volume.",
        confirmLabel: "Remove",
      },
      run: act.remove,
    },
  ];
}

/** What a run's menu runs. */
export interface RunMenuActions {
  open(): void;
  stop(): Promise<unknown>;
}

/** The entries of `run`'s menu. Stable action ids: `run.open`, `run.stop`. */
export function runMenuEntries(run: AgentRunSummary, act: RunMenuActions): ContextMenuEntry[] {
  return [{ id: "run.open", label: "Open", run: act.open }, SEPARATOR, run.status === "running" && { id: "run.stop", label: "Stop", run: act.stop }];
}

/** Props for a persistent agent's row or detail header: its context menu. */
export function useAgentContextMenu(agent: AgentSummary, { open = true }: { open?: boolean } = {}): ContextMenuProps {
  const navigate = useNavigate();
  const { pauseAgent, resumeAgent, removeAgent, stopRun } = useAgents();
  const fail = (word: string) => toastError(`Couldn't ${word} ${agent.name}`);
  const act: AgentMenuActions = {
    open: () => void navigate({ to: "/agents", search: { agent: agent.name }, replace: true }),
    pause: () => pauseAgent(agent.name).catch(fail("pause")),
    resume: () => resumeAgent(agent.name).catch(fail("resume")),
    stop: () => {
      const run = currentRun(agent);
      return run ? stopRun(run.spaceId, run.runId).catch(fail("stop")) : Promise.resolve();
    },
    remove: () =>
      removeAgent(agent.name).then(() => {
        toast(`Removed ${agent.name}`, { type: "success" });
        void navigate({ to: "/agents", search: {}, replace: true });
      }, fail("remove")),
  };
  return useContextMenu(() => agentMenuEntries(agent, act, { open }));
}

/** Props for a run's row: its context menu. */
export function useRunContextMenu(run: AgentRunSummary): ContextMenuProps {
  const navigate = useNavigate();
  const { stopRun } = useAgents();
  return useContextMenu(() =>
    runMenuEntries(run, {
      open: () => void navigate({ to: "/agents", search: { space: run.spaceId, run: run.runId }, replace: true }),
      stop: () => stopRun(run.spaceId, run.runId).catch(toastError("Couldn't stop the run")),
    }),
  );
}
