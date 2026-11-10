// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useMemo } from "react";

import { sharesDesktop, useAgents, useBridge, useSession, useSpaces, type SpacesDefaultHook } from "@/bridge";
import { useChildren } from "@/components/children/use-children";
import { useMachinesLive } from "@/hooks/use-machines-live";
import { currentLocale, useT } from "@/i18n";
import { useHiddenSpaceIds } from "@/lib/prepare/store";
import { buildSpacesTree, type SpacesTree } from "@/lib/spaces-tree";

const NO_RUNS: ReadonlyMap<string, number> = new Map();

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
 * Your computers and the Spaces on them (lib/spaces-tree.ts), with the
 * account's default Space. `running`: the task counts, where the screen shows
 * them.
 */
export function useSpacesTreeData(running: ReadonlyMap<string, number> = NO_RUNS): { tree: SpacesTree | null; loaded: boolean; defaults: SpacesDefaultHook } {
  useT(); // re-renders on a language change: the tree's words are built in it
  const lng = currentLocale();
  const { data: spaces, listStatus } = useSpaces();
  // The sidebar counts the tree on every page, so nothing else asks for the computers again: without this a computer that went
  // offline keeps its green dot (and "Online, Default") while Home already says it is offline.
  const { machines, defaults, relayOnline } = useMachinesLive(true);
  const { data: session } = useSession();
  const { core } = useBridge();
  // The Space being prepared for Home's box is no Space of yours yet.
  const warming = useHiddenSpaceIds(core);
  // The setting names the default; until it's read (or when it names none) the relay's own answer does.
  const defaultSpace = defaults.settings?.defaultSpace ?? defaults.resolved?.defaultSpace ?? null;
  // Which Space made which (Child Spaces): a child sits under its parent. The host's store re-renders this when it reads again.
  const store = useChildren();
  const parentPairs = (spaces ?? []).flatMap((s) => {
    const parent = store?.node(s.id)?.parent;
    return parent ? [`${s.id}\n${parent}`] : [];
  });
  const parentsKey = parentPairs.join("\n\n");
  const tree = useMemo(
    () =>
      spaces && machines
        ? buildSpacesTree({
            spaces: spaces.filter((s) => !warming.has(s.id)),
            machines,
            defaultSpace,
            running,
            me: session?.identity ?? null,
            relayOnline,
            shares: (s) => sharesDesktop(core, s) !== false,
            parents: new Map(parentsKey ? parentsKey.split("\n\n").map((p) => p.split("\n") as [string, string]) : []),
          })
        : null,
    [spaces, machines, warming, defaultSpace, running, session?.identity, relayOnline, lng, parentsKey, core],
  );
  return { tree, loaded: Boolean(spaces && machines && listStatus.loaded), defaults };
}
