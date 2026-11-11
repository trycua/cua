// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Home's pool: your computers as one supercomputer. The total, each
 * computer's free room and where a new job goes are the app core's
 * (`pool.summary`, `pool.place`, libs/cua/crates/cua-spaces-app-core/src/pool.rs),
 * so every app places a job the same way. This file only gathers what the
 * core reads (the account's computers and the numbers each one measured,
 * `resources.get`) and says the result in words.
 *
 * Without the core (a build without the wasm), the stand-ins add nothing
 * up and keep the person's pick, else the first computer that can take a job.
 */

import type { AgentChat, ComputerResources, Machine, Space } from "@/bridge";
import type { CoreClient } from "@/bridge/core";
import { t } from "@/i18n";

import type { TaskWhere } from "./ask-first";
import type { TreeNode } from "./spaces-tree";

/** `pool::PoolResources`. */
export interface PoolResources {
  cpuCount: number;
  cpuPercent: number;
  memoryTotal: number;
  memoryUsed: number;
}

/** `pool::PoolComputer`. */
export interface PoolComputer {
  id: string;
  current: boolean;
  online: boolean;
  notSharing: boolean;
  resources: PoolResources | null;
  jobs: number;
}

/** `pool::Free`. */
export interface Free {
  cores: number;
  memory: number;
  fraction: number;
}

/** `pool::PoolSummary`. */
export interface PoolSummary {
  computers: number;
  online: number;
  cores: number;
  memory: number;
  freeCores: number;
  freeMemory: number;
  unknown: number;
}

export type PlaceReason = "best-fit" | "room" | "unknown" | "you-are-here" | "offline" | "not-sharing";

/** `pool::PlaceRow`. */
export interface PlaceRow {
  id: string;
  eligible: boolean;
  reason: PlaceReason;
  free: Free | null;
}

/** `pool::Placement`. */
export interface Placement {
  chosen: string | null;
  best: string | null;
  movedFrom: string | null;
  rows: PlaceRow[];
}

export interface PlaceInput {
  computers: PoolComputer[];
  picked?: string | null;
  exclude?: string[];
}

const GIB = 1024 ** 3;

/** What the pool reads of a computer's numbers. */
export function poolResources(r: ComputerResources | null | undefined): PoolResources | null {
  if (!r) return null;
  return { cpuCount: r.cpuCount, cpuPercent: r.cpuPercent, memoryTotal: r.memoryTotal, memoryUsed: r.memoryUsed };
}

/** One of Home's computers: the Computers list's row, its numbers and its jobs. */
export interface HomeComputer {
  node: TreeNode;
  machine: Machine;
  resources: ComputerResources | null;
  jobs: AgentChat[];
  /** This computer can't take jobs yet (not set up to share): only for the one you're on. */
  needsSetup?: boolean;
}

export function poolComputer(c: HomeComputer): PoolComputer {
  return {
    id: c.machine.id,
    current: c.machine.current,
    online: c.node.online,
    notSharing: Boolean(c.machine.notSharing) || Boolean(c.needsSetup),
    resources: poolResources(c.resources),
    jobs: c.jobs.length,
  };
}

/** The pool's total (`pool.summary`). */
export function poolSummary(core: CoreClient | null | undefined, computers: readonly PoolComputer[]): PoolSummary {
  const out = core?.tryCall<PoolSummary>("pool.summary", { computers: computers as unknown as Record<string, unknown>[] });
  if (out) return out;
  const online = computers.filter((c) => c.online).length;
  return { computers: computers.length, online, cores: 0, memory: 0, freeCores: 0, freeMemory: 0, unknown: computers.length };
}

/** Where a job goes (`pool.place`). */
export function placeJob(core: CoreClient | null | undefined, input: PlaceInput): Placement {
  const out = core?.tryCall<Placement>("pool.place", { input: { picked: null, exclude: [], ...input } as unknown as Record<string, unknown> });
  if (out) return out;
  const excluded = new Set(input.exclude ?? []);
  const rows: PlaceRow[] = input.computers.map((c) => {
    const ok = c.online && !c.notSharing && !excluded.has(c.id);
    return { id: c.id, eligible: ok, reason: !c.online || excluded.has(c.id) ? "offline" : c.notSharing ? "not-sharing" : "unknown", free: null };
  });
  const best = rows.find((r) => r.eligible)?.id ?? null;
  const pickedOk = input.picked && rows.some((r) => r.id === input.picked && r.eligible) ? input.picked : null;
  const chosen = pickedOk ?? best;
  return { chosen, best, movedFrom: input.picked && !pickedOk && chosen ? input.picked : null, rows };
}

/** "40 GB": memory in whole gigabytes, the way macOS labels it. */
export const gb = (bytes: number): string => t("home.pool.gb", { count: Math.round(bytes / GIB) });

/** The heading's parts, "3 computers", "40 cores", "96 GB" (drawn with dots between); without numbers, just the computers. */
export function poolHeadline(s: PoolSummary): string[] {
  const computers = t("home.pool.computers", { count: s.computers });
  if (!s.cores && !s.memory) return [computers];
  return [computers, t("home.pool.cores", { count: s.cores }), gb(s.memory)];
}

/** "2 online now, with 21 cores and 50 GB free for new jobs." */
export function poolLine(s: PoolSummary): string {
  if (s.online === 0) return t("home.pool.line_none_online");
  if (!s.freeCores && !s.freeMemory) return t("home.pool.line_online", { count: s.online });
  return t("home.pool.line_free", { count: s.online, cores: t("home.pool.cores", { count: s.freeCores }), memory: gb(s.freeMemory) });
}

/** The card's second line's parts: "Mac Studio", "20 cores", "64 GB". */
export function computerSpec(model: string, r: PoolResources | null): string[] {
  if (!r || (!r.cpuCount && !r.memoryTotal)) return [model];
  return [model, t("home.pool.cores", { count: r.cpuCount }), gb(r.memoryTotal)];
}

/** Free now: "12 of 20 cores", "40 GB"; empty when nothing is known or it's offline. */
export function freeText(free: Free | null, r: PoolResources | null): string[] {
  if (!free || !r) return [];
  return [t("home.pool.free_cores", { cores: free.cores, total: r.cpuCount }), gb(free.memory)];
}

/** One "Where" row's note. */
export function reasonText(row: PlaceRow): string {
  switch (row.reason) {
    case "best-fit":
      return row.free ? t("home.run.reason_best_fit", { cores: row.free.cores }) : t("home.run.reason_best_fit_unknown");
    case "room":
      return row.free ? t("home.run.reason_room", { cores: row.free.cores }) : t("home.run.reason_unknown");
    case "unknown":
      return t("home.run.reason_unknown");
    case "you-are-here":
      return t("home.run.reason_here");
    case "offline":
      return t("home.run.reason_offline");
    case "not-sharing":
      return t("home.run.reason_not_sharing");
  }
}

/** The Space ids whose agents count as this computer's jobs: its own desktop and every Space made on it. */
export function computerSpaceIds(node: TreeNode, machine: Pick<Machine, "id" | "current" | "relayId">): Set<string> {
  const ids = new Set<string>();
  if (node.space) ids.add(node.space.id);
  for (const c of node.children) if (c.space) ids.add(c.space.id);
  ids.add(`relay:${machine.id}`);
  if (machine.current) {
    ids.add("this-mac");
    if (machine.relayId) ids.add(`relay:${machine.relayId}`);
  }
  return ids;
}

/** Tones that are a job on a computer now: working, waiting for you, starting or failed to start. */
const ACTIVE: ReadonlySet<AgentChat["tone"]> = new Set(["working", "you", "creating", "not-connected", "failed"]);

/** The agents working on each computer, by machine id; each agent counted once, on its first place. */
export function jobsByComputer(
  chats: readonly AgentChat[] | null | undefined,
  computers: readonly { machine: Machine; ids: Set<string> }[],
): Map<string, AgentChat[]> {
  const out = new Map<string, AgentChat[]>(computers.map((c) => [c.machine.id, []]));
  for (const chat of chats ?? []) {
    if (!ACTIVE.has(chat.tone)) continue;
    const place = chat.places[0];
    if (!place) continue;
    const owner = computers.find((c) => c.ids.has(place.spaceId));
    if (owner) out.get(owner.machine.id)!.push(chat);
  }
  return out;
}

/**
 * Where a job on a computer runs: this computer itself; another computer's own desktop when it shares it; else a
 * new Linux Space made on that computer.
 */
export function whereOn(c: Pick<HomeComputer, "node" | "machine">): TaskWhere {
  if (c.machine.current) return { kind: "this-mac" };
  const desktop: Space | null = c.node.space;
  if (desktop && c.node.online) return { kind: "space", space: desktop };
  return { kind: "new", os: "linux", on: { machine: c.machine.id, name: c.machine.name } };
}
