// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { childCounts, nestRows, parentIdOf, topLevel } from "./space-tree";

const it_ = (id: string, parent?: string) => ({ space: { id, parent } });
const tree = (m: Record<string, string>) => ({ node: (id: string) => (m[id] ? { id, parent: m[id], depth: 1, childrenOn: false } : undefined) }) as never;

describe("the Spaces tree", () => {
  const items = [it_("a"), it_("b"), it_("c", "a"), it_("d", "c"), it_("z", "gone")];

  it("nests children under their parent to any depth, in order, and the rest stays top level", () => {
    const rows = nestRows(items, null, new Set());
    expect(rows.map((r) => [r.item.space.id, r.depth])).toEqual([
      ["a", 0],
      ["c", 1],
      ["d", 2],
      ["b", 0],
      ["z", 0],
    ]);
    expect(rows.find((r) => r.item.space.id === "a")).toMatchObject({ kids: 1, below: 2 });
  });

  it("a collapsed parent hides what is under it, and still counts it", () => {
    const rows = nestRows(items, null, new Set(["a"]));
    expect(rows.map((r) => r.item.space.id)).toEqual(["a", "b", "z"]);
    expect(rows[0]).toMatchObject({ kids: 1, below: 2 });
  });

  it("a child whose parent is gone is at the top with a parent-removed hint; one merely filtered out is not an orphan", () => {
    const rows = nestRows(items, null, new Set());
    expect(rows.find((r) => r.item.space.id === "z")?.orphan).toBe(true);
    const filtered = nestRows([it_("c", "a")], null, new Set(), new Set(["a", "c"]));
    expect(filtered[0]).toMatchObject({ depth: 0, orphan: false });
  });

  it("the host's tree wins over the Space's own parent, and the grid draws the top level only", () => {
    expect(parentIdOf(it_("x", "p").space, tree({ x: "q" }))).toBe("q");
    expect(parentIdOf(it_("x", "p").space, null)).toBe("p");
    expect(topLevel(items, null).map((i) => i.space.id)).toEqual(["a", "b", "z"]);
    expect(childCounts(items, null).get("a")).toBe(1);
  });

  it("a loop in the data cannot hang the list", () => {
    const rows = nestRows([it_("p", "q"), it_("q", "p")], null, new Set());
    expect(rows.map((r) => r.item.space.id).sort()).toEqual(["p", "q"]);
  });
});
