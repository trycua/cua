// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The agent home is a Space like any other (CUA-2630), listed with the person's
 * Spaces. A message typed on Home makes a NEW home every time (cheap: it claims
 * a warm Space); the earlier homes and their agents stay in the list. This
 * remembers, per agent on this device, the newest home (`localStorage`, like the
 * grants), which is the one continued when the person explicitly picks "Let the
 * agent decide, in your existing Home", and lets the Spaces list tag it "Home".
 * Opening a Space (from Your agents or Spaces) and writing in its thread never
 * goes through here: it posts to that Space. Many homes can accumulate (1 CPU and
 * 1 GB each), so idle ones are worth stopping.
 */

import { useSyncExternalStore } from "react";

import type { Space } from "@/bridge";

const KEY = "cua.agentHome.spaces";

/** Agent (harness id) to the id of its home Space. */
type Store = Record<string, string>;

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

/** The id of the Space that is `agent`'s home, if one was made. */
export function homeSpaceId(agent: string): string | null {
  return load()[agent] ?? null;
}

export function rememberHome(agent: string, spaceId: string): void {
  save({ ...load(), [agent]: spaceId });
}

export function forgetHome(agent: string): void {
  const { [agent]: _gone, ...rest } = load();
  save(rest);
}

/** For tests: forget the cached homes. */
export function resetHomes(): void {
  cache = null;
}

/** Whether `spaceId` is a remembered home. */
export function isHomeSpaceId(spaceId: string): boolean {
  return Object.values(load()).includes(spaceId);
}

/** A Space as the home lookup reads it. */
export type HomeCandidate = Pick<Space, "id" | "status"> & Partial<Pick<Space, "lastUsedAt">>;

/**
 * The home to reuse for `agent` among the listed Spaces: still there and not broken. `fresh` asks for a
 * new one. The one this device remembers wins; with none (a new device, cleared storage) it is a Space
 * the host says is a home (`labelled`: ids from the Spaces' `cua.home` labels, the ones made for `agent`
 * first). A home is therefore found again with no help from this device.
 */
export function reusableHome(agent: string, spaces: readonly HomeCandidate[] | undefined, fresh = false, labelled: readonly string[] = []): string | null {
  if (fresh) return null;
  // A list that doesn't show it any more means it was removed.
  const alive = (id: string) => spaces?.find((s) => s.id === id && s.status !== "deleting");
  const id = homeSpaceId(agent);
  if (id && alive(id)) return id;
  const found = labelled.map(alive).find((s) => s !== undefined);
  return found ? found.id : null;
}

/**
 * The home a Home-composer request goes to: none (a new home is made) unless the person picked "Let the
 * agent decide, in your existing Home" (`where.reuse`). Naming a Space or a computer, or any other place, never reuses.
 */
export function homeForRequest(
  where: { kind: string; reuse?: boolean },
  agent: string,
  spaces: readonly HomeCandidate[] | undefined,
  labelled: readonly string[] = [],
): string | null {
  const continuing = where.kind === "agent" && Boolean(where.reuse);
  return reusableHome(agent, spaces, !continuing, labelled);
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

/** Whether this Space is an agent's home (the Spaces list tags it "Home"). */
export function useIsHomeSpace(spaceId: string): boolean {
  return useSyncExternalStore(
    subscribe,
    () => isHomeSpaceId(spaceId),
    () => false,
  );
}
