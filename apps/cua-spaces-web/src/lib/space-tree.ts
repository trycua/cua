// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Where a Space sits in the parent/child tree the Spaces page shows: a child nests under its parent
 * (indented, collapsible, to any depth); a child whose parent is gone stands at the top level with a quiet
 * "parent removed" hint. The parent comes from the host's tree (`ChildrenPreview.node`), else from the Space's
 * own `parent` (a Space still being made has no tree node yet).
 */

import type { ChildrenPreview } from "@/bridge/contracts/children";

interface Item {
  space: { id: string; parent?: string };
}

/** `space`'s parent id as the tree or the Space says it, if any (whether or not that parent is listed). */
export function parentIdOf(space: Item["space"], tree: Pick<ChildrenPreview, "node"> | null): string | undefined {
  return tree?.node(space.id)?.parent ?? space.parent ?? undefined;
}

export interface TreeRow<T extends Item> {
  item: T;
  depth: number;
  /** Children listed under it (0: no disclosure). */
  kids: number;
  /** All the Spaces under it, at any depth (what Delete would take with it). */
  below: number;
  /** Its parent is set but is not in `known`: it stands at the top with "parent removed". */
  orphan: boolean;
}

/**
 * `items` as the rows of a tree: each Space's listed children follow it (in `items`' order), only under the
 * ones not in `closed`. `known` is every Space the account has, so a parent that is merely filtered out of
 * `items` is not mistaken for a removed one.
 */
export function nestRows<T extends Item>(
  items: readonly T[],
  tree: Pick<ChildrenPreview, "node"> | null,
  closed: ReadonlySet<string>,
  known: ReadonlySet<string> = new Set(items.map((i) => i.space.id)),
): TreeRow<T>[] {
  const listed = new Set(items.map((i) => i.space.id));
  const listedParent = (i: T): string | undefined => {
    const p = parentIdOf(i.space, tree);
    return p && p !== i.space.id && listed.has(p) ? p : undefined;
  };
  const under = new Map<string, T[]>();
  for (const i of items) {
    const p = listedParent(i);
    if (p) under.set(p, [...(under.get(p) ?? []), i]);
  }
  const count = (id: string, seen: Set<string>): number => {
    if (seen.has(id)) return 0;
    seen.add(id);
    return (under.get(id) ?? []).reduce((n, k) => n + 1 + count(k.space.id, seen), 0);
  };
  const out: TreeRow<T>[] = [];
  const walk = (i: T, depth: number, seen: Set<string>) => {
    if (seen.has(i.space.id)) return;
    seen.add(i.space.id);
    const kids = under.get(i.space.id) ?? [];
    const p = parentIdOf(i.space, tree);
    out.push({ item: i, depth, kids: kids.length, below: count(i.space.id, new Set()), orphan: depth === 0 && !!p && !known.has(p) });
    if (!closed.has(i.space.id) && depth < 8) for (const k of kids) walk(k, depth + 1, seen);
  };
  const seen = new Set<string>();
  for (const i of items) if (!listedParent(i)) walk(i, 0, seen);
  // A loop in the data (a is under b, b under a): whatever no row reaches, folded or not, still shows at the top.
  const covered = new Set(seen);
  const cover = (id: string) => {
    for (const k of under.get(id) ?? []) if (!covered.has(k.space.id)) (covered.add(k.space.id), cover(k.space.id));
  };
  for (const id of [...seen]) cover(id);
  for (const i of items) if (!covered.has(i.space.id)) walk(i, 0, seen);
  return out;
}

/** The Spaces the grid draws as tiles: the top level (children are under their parent, in the list and its page). */
export function topLevel<T extends Item>(items: readonly T[], tree: Pick<ChildrenPreview, "node"> | null): T[] {
  const listed = new Set(items.map((i) => i.space.id));
  return items.filter((i) => {
    const p = parentIdOf(i.space, tree);
    return !(p && p !== i.space.id && listed.has(p));
  });
}

/** How many Spaces sit directly under each parent in `items`. */
export function childCounts<T extends Item>(items: readonly T[], tree: Pick<ChildrenPreview, "node"> | null): Map<string, number> {
  const out = new Map<string, number>();
  const listed = new Set(items.map((i) => i.space.id));
  for (const i of items) {
    const p = parentIdOf(i.space, tree);
    if (p && p !== i.space.id && listed.has(p)) out.set(p, (out.get(p) ?? 0) + 1);
  }
  return out;
}
