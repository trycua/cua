// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The Spaces page's rules that are not the core's: what its URL holds,
 * which Spaces it shows in which order, the corner badge on a tile and the
 * "Last used" words of the list (CUA-2204).
 */

import type { Machine, Space } from "@/bridge";
import { thisComputerLabel } from "@/lib/host-labels";
import { size } from "@/lib/resources";
import { spaceState, type SpaceFilter, type SpaceState } from "@/lib/spaces";

export type SpacesView = "grid" | "list";
export type SpacesSort = "used" | "name" | "size";
export type SpacesOwner = "mine" | "shared";

/** What the Spaces page keeps in its URL; every field is optional (the default is left out). */
export interface SpacesSearch {
  /** `list`: the list view (grid otherwise). */
  view?: "list";
  /** `name` or `size` (biggest first); last used otherwise. */
  sort?: "name" | "size";
  /** Only the Spaces on this computer (a `machines.list` id). */
  on?: string;
  state?: Exclude<SpaceFilter, "all">;
  /** `mine`: yours; `shared`: shared with you or a team's. */
  owner?: SpacesOwner;
  /** `stopped`: select every stopped Space shown, once, when the page opens. */
  select?: "stopped";
}

const pick = <T extends string>(v: unknown, ok: readonly T[]): T | undefined => (ok.includes(v as T) ? (v as T) : undefined);

/** The route's `validateSearch`: anything else in the URL is dropped. */
export function validateSpacesSearch(s: Record<string, unknown>): SpacesSearch {
  const out: SpacesSearch = {
    view: pick(s.view, ["list"] as const),
    sort: pick(s.sort, ["name", "size"] as const),
    on: typeof s.on === "string" && s.on !== "" ? s.on : undefined,
    state: pick(s.state, ["running", "stopped", "creating"] as const),
    owner: pick(s.owner, ["mine", "shared"] as const),
    select: pick(s.select, ["stopped"] as const),
  };
  return Object.fromEntries(Object.entries(out).filter(([, v]) => v !== undefined)) as SpacesSearch;
}

/** `prev` with `next` applied, defaults left out of the URL. */
export function nextSearch(prev: SpacesSearch, next: Partial<Record<keyof SpacesSearch, string | undefined>>): SpacesSearch {
  return validateSpacesSearch({ ...prev, ...next });
}

/** Which of your computers a Space is on (a `machines.list` id); undefined for a cloud Space or someone else's computer. */
export function spaceMachineId(space: Pick<Space, "id" | "provider" | "host">, machines: readonly Pick<Machine, "id" | "current" | "spaceIds">[]): string | undefined {
  if (space.provider === "local") return machines.find((m) => m.current)?.id;
  if (space.host && machines.some((m) => m.id === space.host)) return space.host;
  if (space.provider === "relay" && space.id.startsWith("relay:")) {
    const own = space.id.slice("relay:".length).split("/")[0];
    if (own && machines.some((m) => m.id === own)) return own;
  }
  return machines.find((m) => m.spaceIds.includes(space.id))?.id;
}

/** The computers the "Any computer" menu offers: This Mac first, then each machine (never a bare device). */
export function computerChoices(machines: readonly Machine[]): { value: string; label: string }[] {
  const real = machines.filter((m) => !m.device);
  const here = real.filter((m) => m.current);
  return [...here, ...real.filter((m) => !m.current)].map((m) => ({ value: m.id, label: m.current ? thisComputerLabel() : m.name }));
}

const ORDER: Record<SpaceState, number> = { failed: 0, creating: 0, running: 1, stopped: 2, deleting: 3 };

export interface ShownInput {
  spaces: readonly Space[];
  search: SpacesSearch;
  /** Yours (owner filter). */
  mine: (space: Space) => boolean;
  /** Its computer's id (computer filter). */
  machineOf: (space: Space) => string | undefined;
  /** Its size on disk, null when not known (size sort). */
  bytes: (space: Space) => number | null;
  /** The filter box keeps it. */
  matches: (space: Space) => boolean;
}

/**
 * The Spaces the page shows, in order. Last used: in the grid, being
 * created first, then running, then the rest, most recent first in each
 * (as before); in the list strictly by the column. Name: A to Z. Size:
 * biggest first, the ones without a known size last.
 */
export function shownSpaces({ spaces, search, mine, machineOf, bytes, matches }: ShownInput): Space[] {
  const filter = search.state ?? "all";
  const kept = spaces.filter(
    (s) =>
      (filter === "all" || spaceState(s) === filter) &&
      (!search.on || machineOf(s) === search.on) &&
      (!search.owner || (search.owner === "mine") === mine(s)) &&
      matches(s),
  );
  const byUsed = (a: Space, b: Space) => b.lastUsedAt - a.lastUsedAt;
  const byName = (a: Space, b: Space) => a.name.localeCompare(b.name);
  if (search.sort === "name") return kept.toSorted((a, b) => byName(a, b) || byUsed(a, b));
  if (search.sort === "size") return kept.toSorted((a, b) => (bytes(b) ?? -1) - (bytes(a) ?? -1) || byName(a, b));
  if (search.view === "list") return kept.toSorted(byUsed);
  return kept.toSorted((a, b) => ORDER[spaceState(a)] - ORDER[spaceState(b)] || byUsed(a, b));
}

/** The list's Size column: yours with a known size, else "—". */
export function sizeCell(bytes: number | null, mine: boolean): string {
  return mine && bytes != null && bytes > 0 ? size(bytes) : "—";
}

/** The list's "Last used": "Just now", "2 min ago", "3 hr ago", "Yesterday", "9 days ago", "3 weeks ago". */
export function lastUsedText(ms: number, now: number): string {
  if (!ms) return "—";
  const m = Math.max(0, Math.floor((now - ms) / 60_000));
  if (m < 1) return "Just now";
  if (m < 60) return `${m} min ago`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h} hr ago`;
  const d = Math.floor(h / 24);
  if (d < 2) return "Yesterday";
  if (d < 14) return `${d} days ago`;
  const w = Math.floor(d / 7);
  if (d < 60) return `${w} weeks ago`;
  const mo = Math.floor(d / 30);
  return mo < 12 ? `${mo} months ago` : "Over a year ago";
}

/** "Sam", "Sam and Priya", "Sam, Priya and 3 others". */
export function namesText(list: readonly string[]): string {
  if (list.length <= 2) return list.join(" and ");
  const rest = list.length - 2;
  return `${list.slice(0, 2).join(", ")} and ${rest} ${rest === 1 ? "other" : "others"}`;
}

/** An avatar's letter: the first letter of a name or address. */
export function initial(name: string): string {
  return (name.trim()[0] ?? "?").toUpperCase();
}

const AVATAR_COLORS = ["bg-violet-500", "bg-amber-600", "bg-teal-600", "bg-sky-600", "bg-rose-500", "bg-emerald-600", "bg-indigo-500", "bg-orange-500"];

/** The same person or team gets the same color everywhere. */
export function avatarColor(key: string): string {
  let h = 0;
  for (const c of key.toLowerCase()) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return AVATAR_COLORS[h % AVATAR_COLORS.length]!;
}

/** What stands in the grid's place, under the toolbar. */
export type GridArea = "error" | "loading" | "guide" | "empty" | "noMatches" | "noneInFilter" | "spaces";

/**
 * The grid's place, from what the list says now: the Spaces, or why there
 * are none (it could not be read, it is still loading, the first-run guide,
 * none yet, none matching the filter box, none in the chosen filters).
 */
export function gridArea(s: { error: boolean; loading: boolean; guide: boolean; total: number; shown: number; query: string }): GridArea {
  if (s.error) return "error";
  if (s.loading) return "loading";
  if (s.guide) return "guide";
  if (s.total === 0) return "empty";
  if (s.shown === 0) return s.query.trim() ? "noMatches" : "noneInFilter";
  return "spaces";
}
