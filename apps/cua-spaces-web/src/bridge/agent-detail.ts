// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * One persistent agent's Memory, Routines and Access to this machine, as
 * the SwiftUI app's Agents page draws them (`AgentsPageView`). Every word
 * and rule is the app core's (`agents.pageReduce` / `agents.pageView`,
 * `cua-spaces-app-core/src/persistent.rs`); this runs the command each
 * action asks for through `ops/agent-detail.ts` and keeps the answers, as
 * `PersistentModel.send` does natively.
 *
 * Without the core there is no detail (`view` stays null): the page shows
 * the agent's run only.
 */

import { useCallback, useEffect, useRef, useState } from "react";

import { isUnsupported, type DataAdapter } from "./adapter";
import type { CoreClient } from "./core";
import type {
  AgentDetailView,
  AgentsPageAction,
  AgentsPageInput,
  AgentsPageRequest,
  AgentsPageState,
  AgentsPageView,
  PersistentAgent,
} from "./contracts/agents";
import { useBridge } from "./hooks";

export function agentsPageReduce(core: CoreClient, input: AgentsPageInput, state: AgentsPageState, action: AgentsPageAction): AgentsPageState {
  return core.call<AgentsPageState>("agents.pageReduce", { input, state, action });
}

export function agentsPageDetail(core: CoreClient, input: AgentsPageInput, state: AgentsPageState, nowMs: number): AgentsPageView | null {
  return core.tryCall<AgentsPageView>("agents.pageView", { input, state, nowMs }) ?? null;
}

/** Runs one request against the host and answers what it read into the input. */
export async function runAgentsRequest(data: DataAdapter, request: AgentsPageRequest, selected: string | null): Promise<Partial<AgentsPageInput>> {
  const reload = async () => (selected ? await data.call("agents.detail", { name: selected }).catch(() => ({})) : {});
  switch (request.kind) {
    case "load":
      return data.call("agents.detail", { name: request.name });
    case "read-file":
      return { file: await data.call("agents.readFile", { path: request.path }) };
    case "pause":
      await data.call("agents.pause", { name: request.name });
      return {};
    case "resume":
      await data.call("agents.resume", { name: request.name });
      return {};
    case "restore":
      await data.call("agents.restoreFile", { path: request.path, version: request.version });
      return reload();
    case "add-routine":
      await data.call("agents.addRoutine", {
        agent: request.agent,
        title: request.title,
        prompt: request.prompt,
        everyMinutes: request.every_minutes,
        dailyAt: request.daily_at,
        weeklyOn: request.weekly_on,
      });
      return reload();
    case "set-routine":
      await data.call("agents.setRoutine", { id: request.id, enabled: request.enabled });
      return reload();
    case "remove-routine":
      await data.call("agents.removeRoutine", { id: request.id });
      return reload();
    case "allow":
      await data.call("agents.allowComputer", { agent: request.agent, machine: request.machine });
      return reload();
    case "revoke":
      await data.call("agents.revokeComputer", { agent: request.agent, machine: request.machine });
      return reload();
  }
}

export interface AgentDetailHook {
  /** The agent as the core draws it; null until its first read, or without the core. */
  view: AgentDetailView | null;
  /** A command is running (`request` in the core's words, `requestText`). */
  busy: boolean;
  /** Why the last command failed. */
  error: string | null;
  /** The host can't read an agent's detail. */
  unsupported: boolean;
  /** This machine's Space id, which "Allow on this machine" grants. */
  thisMachine: string | null;
  /** Sends a page action; runs the command the core asks for. */
  act(action: AgentsPageAction): void;
}

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** `agent`'s detail while mounted (the SwiftUI `AgentsPageView`'s detail section). */
export function useAgentDetail(agent: PersistentAgent | null, now: () => number = Date.now): AgentDetailHook {
  const { core, data } = useBridge();
  const ready = core.status === "ready" && core.methods.includes("agents.pageReduce");
  const [input, setInput] = useState<Omit<AgentsPageInput, "agents">>({});
  const [state, setState] = useState<AgentsPageState | null>(null);
  const [unsupported, setUnsupported] = useState(false);
  const current = useRef({ input, state, agent });
  current.current = { input, state, agent };

  const typed = (i: Omit<AgentsPageInput, "agents">): AgentsPageInput => ({ ...i, agents: current.current.agent ? [current.current.agent] : [] });

  const act = useCallback(
    (action: AgentsPageAction) => {
      const before = current.current.state;
      if (!ready || !data || !before) return;
      const next = agentsPageReduce(core, typed(current.current.input), before, action);
      current.current.state = next;
      setState(next);
      const request = next.request;
      if (before.busy || !request) return;
      const finish = (a: AgentsPageAction) => {
        const s = agentsPageReduce(core, typed(current.current.input), current.current.state!, a);
        current.current.state = s;
        setState(s);
      };
      runAgentsRequest(data, request, next.selected ?? null).then(
        (read) => {
          const merged = { ...current.current.input, ...read };
          current.current.input = merged;
          setInput(merged);
          finish({ type: "done" });
        },
        (e: unknown) => {
          if (request.kind === "load" && isUnsupported(e)) setUnsupported(true);
          finish({ type: "failed", error: message(e) });
        },
      );
    },
    // `typed` reads the refs.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [core, data, ready],
  );

  // A new page per agent, then its first read (the core's `select`).
  const name = agent?.name ?? null;
  useEffect(() => {
    if (!ready || !name) return;
    const initial = core.call<AgentsPageState>("agents.pageInitial", {});
    current.current = { ...current.current, input: {}, state: initial };
    setInput({});
    setState(initial);
    setUnsupported(false);
    act({ type: "select", name });
  }, [ready, core, name, act]);

  const page = ready && state && agent ? agentsPageDetail(core, { ...input, agents: [agent] }, state, now()) : null;
  return {
    view: page?.detail ?? null,
    busy: state?.busy ?? false,
    error: page?.error ?? null,
    unsupported,
    thisMachine: input.thisMachine ?? null,
    act,
  };
}
