// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useMemo } from "react";

import { hostResources, useAgentChats, useBridge, useResources, type AgentChat } from "@/bridge";
import { useRunningBySpace, useSpacesTreeData } from "@/components/spaces/use-spaces-tree";
import { useThisMacTarget, type ThisMacTarget } from "@/components/ask-first/use-this-mac-target";
import { useAgentAttempts } from "@/hooks/use-agent-attempts";
import { computerSpaceIds, jobsByComputer, poolComputer, poolSummary, type HomeComputer, type PoolComputer, type PoolSummary } from "@/lib/pool";

export interface HomeComputers {
  /** Null until your computers have loaded. */
  computers: HomeComputer[] | null;
  pool: PoolComputer[];
  summary: PoolSummary;
  chats: AgentChat[] | null;
  thisMac: ThisMacTarget;
}

/**
 * Home's computers: the Computers page's own list (`useSpacesTreeData`), each with the numbers it measured
 * (`resources.get`) and the agents working on it (the Agents page's list), added up by the app core.
 */
export function useHomeComputers(): HomeComputers {
  const running = useRunningBySpace();
  const { tree, loaded } = useSpacesTreeData(running);
  const snapshot = useResources();
  const attempts = useAgentAttempts();
  const { chats } = useAgentChats(attempts);
  const thisMac = useThisMacTarget();
  const { core } = useBridge();
  const computers = useMemo(() => {
    if (!tree || !loaded) return null;
    const rows = tree.roots.filter((n) => n.kind === "computer" && n.machine && !n.machine.device).map((node) => ({ node, machine: node.machine! }));
    const withIds = rows.map((r) => ({ ...r, ids: computerSpaceIds(r.node, r.machine) }));
    const jobs = jobsByComputer(chats, withIds);
    return rows.map(
      ({ node, machine }): HomeComputer => ({
        node,
        machine,
        resources: hostResources(snapshot, machine),
        jobs: jobs.get(machine.id) ?? [],
        ...(machine.current && thisMac.readiness !== "ready" ? { needsSetup: true } : {}),
      }),
    );
  }, [tree, loaded, snapshot, chats, thisMac.readiness]);
  const pool = useMemo(() => (computers ?? []).map(poolComputer), [computers]);
  const summary = useMemo(() => poolSummary(core, pool), [core, pool]);
  return { computers, pool, summary, chats, thisMac };
}
