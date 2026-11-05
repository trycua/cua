// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect, useMemo } from "react";

import { useAgents, useBridge, useMachines, useSession, useSpaces, useSpacesDefault, useSpacesTree, type SpacesDefaultHook } from "@/bridge";
import { useHiddenSpaceIds } from "@/lib/prepare/store";
import { buildSpacesTree, type SpacesTree } from "@/lib/spaces-tree";

const NO_RUNS: ReadonlyMap<string, number> = new Map();

/** How often the tree reads the computers again while it is on (the host answers from memory and asks the relay only when its copy is stale). */
export const MACHINES_REFRESH_MS = 30_000;

/** Running agent tasks by Space id, from the runs in your running Spaces. */
export function useRunningBySpace(): ReadonlyMap<string, number> {
  const { data } = useAgents();
  return useMemo(() => {
    const out = new Map<string, number>();
    for (const r of data?.runs ?? []) if (r.status === "running") out.set(r.spaceId, (out.get(r.spaceId) ?? 0) + 1);
    return out;
  }, [data?.runs]);
}

/**
 * The Spaces tree (lib/spaces-tree.ts) with the account's default Space, while
 * Labs' "Spaces as a tree" is on (`on`); nothing is read while it's off.
 * `running`: the task counts, where the screen shows them.
 */
export function useSpacesTreeData(running: ReadonlyMap<string, number> = NO_RUNS): { on: boolean; tree: SpacesTree | null; loaded: boolean; defaults: SpacesDefaultHook } {
  const on = useSpacesTree();
  const { data: spaces, listStatus } = useSpaces();
  const { data: machines, refresh: refreshMachines } = useMachines();
  // The sidebar counts the tree on every page, so nothing else asks for the computers again: without this a computer that went
  // offline keeps its green dot (and "Online, Default") while Home already says it is offline.
  useEffect(() => {
    if (!on) return;
    const t = setInterval(() => void refreshMachines().catch(() => {}), MACHINES_REFRESH_MS);
    return () => clearInterval(t);
  }, [on]);
  const { data: session } = useSession();
  const { core } = useBridge();
  // The Space being prepared for Home's box is no Space of yours yet.
  const warming = useHiddenSpaceIds(core);
  const defaults = useSpacesDefault(on);
  // The setting names the default; until it's read (or when it names none) the relay's own answer does.
  const defaultSpace = defaults.settings?.defaultSpace ?? defaults.resolved?.defaultSpace ?? null;
  const order = defaults.resolved?.order;
  const relayOnline = useMemo(() => new Map((order ?? []).filter((o) => o.role !== "here").map((o) => [o.space, o.online] as const)), [order]);
  const tree = useMemo(
    () =>
      on && spaces && machines
        ? buildSpacesTree({ spaces: spaces.filter((s) => !warming.has(s.id)), machines, defaultSpace, running, me: session?.identity ?? null, relayOnline })
        : null,
    [on, spaces, machines, warming, defaultSpace, running, session?.identity, relayOnline],
  );
  return { on, tree, loaded: Boolean(spaces && machines && listStatus.loaded), defaults };
}
