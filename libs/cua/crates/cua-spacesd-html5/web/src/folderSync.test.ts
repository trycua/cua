// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { conflictName, emptyBase, ignored, loadBase, plan, storeBase, type Base, type Snapshot } from "./folderSync";

const snap = (files: Record<string, [number, number]>, dirs: string[] = []): Snapshot => ({
  files: new Map(Object.entries(files).map(([p, [size, mtime]]) => [p, { size, mtime }])),
  dirs: new Set(dirs),
});
const base = (files: Record<string, [[number, number], [number, number]]>, dirs: string[] = []): Base => ({
  files: new Map(
    Object.entries(files).map(([p, [l, r]]) => [p, { local: { size: l[0], mtime: l[1] }, remote: { size: r[0], mtime: r[1] } }]),
  ),
  dirs: new Set(dirs),
});
const kinds = (actions: ReturnType<typeof plan>) => actions.map((a) => `${a.kind} ${a.path}`);

describe("folder sync planner", () => {
  it("copies new files both ways on the first pass", () => {
    const actions = plan(snap({ "a.txt": [1, 10] }, ["docs"]), snap({ "b.txt": [2, 20] }), emptyBase());
    expect(kinds(actions)).toEqual(["push a.txt", "pull b.txt", "mkdir-remote docs"]);
  });

  it("adopts equal files by content and flags different ones", () => {
    expect(kinds(plan(snap({ x: [5, 1] }), snap({ x: [5, 9] }), emptyBase()))).toEqual(["compare x"]);
    expect(kinds(plan(snap({ x: [5, 1] }), snap({ x: [6, 9] }), emptyBase()))).toEqual(["conflict x"]);
  });

  it("propagates one-sided edits and deletions", () => {
    const b = base({ a: [[1, 10], [1, 100]], d: [[1, 10], [1, 100]], e: [[1, 10], [1, 100]] });
    const local = snap({ a: [2, 11], e: [1, 10] }); // a edited, d deleted locally
    const remote = snap({ a: [1, 100], d: [1, 100] }); // e deleted in the guest
    expect(kinds(plan(local, remote, b))).toEqual(["push a", "delete-remote d", "delete-local e"]);
  });

  it("keeps the change when one side deleted and the other edited", () => {
    const b = base({ a: [[1, 10], [1, 100]] });
    expect(kinds(plan(snap({}, ["x"]), snap({ a: [3, 200] }), b))).toContain("pull a");
    expect(kinds(plan(snap({ a: [3, 11] }), snap({}, ["x"]), b))).toContain("push a");
  });

  it("reports a conflict when both sides changed", () => {
    const b = base({ a: [[1, 10], [1, 100]] });
    expect(kinds(plan(snap({ a: [2, 11] }), snap({ a: [3, 101] }), b))).toEqual(["conflict a"]);
  });

  it("never deletes from a side that suddenly looks wiped", () => {
    const b = base({ a: [[1, 10], [1, 100]], c: [[1, 10], [1, 100]] });
    expect(kinds(plan(snap({}), snap({ a: [1, 100], c: [1, 100] }), b))).toEqual([]);
    expect(kinds(plan(snap({ a: [1, 10], c: [1, 10] }), snap({}), b))).toEqual([]);
  });

  it("forgets files deleted on both sides and handles empty directories", () => {
    const b = base({ gone: [[1, 1], [1, 1]] }, ["old", "keep"]);
    const actions = plan(snap({ z: [1, 1] }, ["keep"]), snap({ z: [1, 1] }, ["old"]), b);
    // Both dirs were known: each side's deletion is propagated.
    expect(kinds(actions)).toEqual(["forget gone", "compare z", "rmdir-remote old", "rmdir-local keep"]);
  });

  it("names conflict copies next to the file and ignores OS litter", () => {
    const at = new Date(2026, 8, 24, 10, 15, 0);
    expect(conflictName("docs/report.txt", at)).toBe("docs/report (sandbox copy 2026-09-24 101500).txt");
    expect(conflictName("Makefile", at)).toBe("Makefile (sandbox copy 2026-09-24 101500)");
    expect(ignored(".DS_Store")).toBe(true);
    expect(ignored("notes.md")).toBe(false);
  });

  it("round-trips the base for storage", () => {
    const b = base({ a: [[1, 2], [3, 4]] }, ["d"]);
    const again = loadBase(JSON.parse(JSON.stringify(storeBase(b))));
    expect(again.files.get("a")).toEqual({ local: { size: 1, mtime: 2 }, remote: { size: 3, mtime: 4 } });
    expect(again.dirs.has("d")).toBe(true);
  });
});
