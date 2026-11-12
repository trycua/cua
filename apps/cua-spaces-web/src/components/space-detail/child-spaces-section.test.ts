// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { childrenEffect, childrenOfSpace, deleteAllText, descendantCount } from "./child-spaces-section";

const sp = (id: string, parent?: string, name = id) => ({ id, name, parent }) as never;

describe("a Space's child Spaces", () => {
  const spaces = [sp("home"), sp("a", "home"), sp("b", "home"), sp("a1", "a"), sp("other")];

  it("lists the direct children, and counts everything under it", () => {
    expect(childrenOfSpace(spaces, "home", null).map((s) => s.id)).toEqual(["a", "b"]);
    expect(descendantCount(spaces, "home", null)).toBe(3);
    expect(descendantCount(spaces, "other", null)).toBe(0);
  });

  it("says plainly what Delete all takes with it, and that the parent stays", () => {
    const kids = childrenOfSpace(spaces, "home", null);
    const t = deleteAllText("home", kids, descendantCount(spaces, "home", null));
    expect(t.title).toBe("Delete all 2 child Spaces of home?");
    expect(t.description).toContain("a, b are deleted for good");
    expect(t.description).toContain("1 Space made under them go too.");
    expect(t.description).toContain("home stays.");
  });

  it("stopping a parent says its children keep running; deleting it says they go too, and how many are deeper", () => {
    const kids = childrenOfSpace(spaces, "home", null);
    expect(childrenEffect("stop", kids, 3)).toBe(" Its 2 child Spaces (a, b) keep running. Stop them from the Child Spaces list if you want.");
    expect(childrenEffect("delete", kids, 3)).toBe(" Its 2 child Spaces (a, b) are deleted with it, and so is the 1 Space made under them.");
    expect(childrenEffect("delete", [kids[1]!], 1)).toBe(" Its 1 child Space (b) is deleted with it.");
    expect(childrenEffect("stop", [], 0)).toBe("");
  });

  it("a loop does not hang the count", () => {
    expect(descendantCount([sp("p", "q"), sp("q", "p")], "p", null)).toBe(2);
  });
});
