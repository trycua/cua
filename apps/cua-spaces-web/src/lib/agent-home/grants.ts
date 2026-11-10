// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * What the person has approved for each agent, remembered per agent on this
 * device (`localStorage`, like the ask-first tasks): "list all my Spaces"
 * once, "use this Mac" once per machine. A new home for the same agent is
 * made with them, so the agent is not asked again. Revoking takes effect for
 * the next home.
 */

import { useSyncExternalStore } from "react";

import { NO_GRANTS, type AgentGrants } from "./abilities";

const KEY = "cua.agentHome.grants";

type Store = Record<string, AgentGrants>;

let cache: Store | null = null;
const listeners = new Set<() => void>();

function load(): Store {
  if (cache) return cache;
  try {
    const raw = globalThis.localStorage?.getItem(KEY);
    const parsed = raw ? (JSON.parse(raw) as unknown) : {};
    cache = parsed && typeof parsed === "object" && !Array.isArray(parsed) ? (parsed as Store) : {};
  } catch {
    cache = {};
  }
  return cache;
}

function save(next: Store): void {
  cache = next;
  try {
    globalThis.localStorage?.setItem(KEY, JSON.stringify(next));
  } catch {
    // Kept for this session only.
  }
  for (const l of [...listeners]) l();
}

/** What `agent` (a harness id) may do. */
export function grantsFor(agent: string): AgentGrants {
  const g = load()[agent];
  return g ? { listAll: Boolean(g.listAll), machines: Array.isArray(g.machines) ? [...g.machines] : [] } : { ...NO_GRANTS, machines: [] };
}

export function allowListAll(agent: string, on = true): void {
  save({ ...load(), [agent]: { ...grantsFor(agent), listAll: on } });
}

export function allowMachine(agent: string, machine: string, on = true): void {
  const g = grantsFor(agent);
  const machines = on ? [...new Set([...g.machines, machine])] : g.machines.filter((m) => m !== machine);
  save({ ...load(), [agent]: { ...g, machines } });
}

/** For tests: forget the cached grants. */
export function resetGrants(): void {
  cache = null;
}

const subscribe = (l: () => void) => {
  listeners.add(l);
  const onStorage = (e: StorageEvent) => {
    if (e.key === null || e.key === KEY) {
      cache = null;
      l();
    }
  };
  globalThis.addEventListener?.("storage", onStorage);
  return () => {
    listeners.delete(l);
    globalThis.removeEventListener?.("storage", onStorage);
  };
};

export function useGrants(agent: string): AgentGrants {
  // The snapshot is the stored object itself, so it only changes when a grant does.
  const stored = useSyncExternalStore(
    subscribe,
    () => load()[agent],
    () => undefined,
  );
  return stored ? { listAll: Boolean(stored.listAll), machines: stored.machines ?? [] } : NO_GRANTS;
}

/**
 * What this machine's daemon is told when the person approves something
 * mid-run: every id and name of the machines the agent may use, so it
 * matches however the agent names one (`children/home.rs`).
 */
export function homeGrantsArg(g: AgentGrants, machines: ReadonlyMap<string, string>): { listAll: boolean; machines: string[] } {
  const out = new Set<string>();
  for (const id of g.machines) {
    out.add(id);
    const name = machines.get(id);
    if (name) out.add(name);
  }
  return { listAll: g.listAll, machines: [...out].sort() };
}

/**
 * What an @mention in a request to a home that already runs gives it: every
 * id and name of the Spaces and machines the person named, for the daemon to
 * match however the agent names one. An @mention counts as the grant, so no
 * approval card is asked for it (`children/home.rs`, `Grants.mentions`).
 */
export function homeMentionsArg(mentions: { spaces: readonly { id: string; name: string }[]; machines: readonly { id: string; name: string }[] }): string[] {
  return [...new Set([...mentions.spaces, ...mentions.machines].flatMap((m) => [m.id, m.name]).filter(Boolean))].sort();
}
