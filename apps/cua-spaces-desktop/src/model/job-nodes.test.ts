// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { jobNodes, type SpacesBackend, type TreeNodeJson } from "./backend";
import { LiveGate, PendingSpacesBackend, type LiveServices } from "./pending";

const parent: TreeNodeJson = { id: "local:home", parent: null, depth: 1, children: true, home: "claude" };

function homeWith(rows: unknown): string {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), "jobrows-"));
  fs.mkdirSync(path.join(home, "spawn"));
  fs.writeFileSync(path.join(home, "spawn", "job-rows.json"), JSON.stringify(rows));
  return home;
}

describe("jobNodes: one logical child per run, wherever it runs", () => {
  it("puts a job on another host under its submitting Space, one level deeper", () => {
    const home = homeWith([
      { run: "p-1", parent: "local:home", name: "quiet-orca", gpu: true, host: "mac-m2max", state: "running", members: 1 },
    ]);
    const [n] = jobNodes([parent], home);
    expect(n).toMatchObject({ id: "job:p-1", parent: "local:home", depth: 2, children: false });
    expect(n.job).toMatchObject({ name: "quiet-orca", gpu: true, host: "mac-m2max", state: "running", members: 1 });
  });
  it("keeps a gang as one row with its member count, and a failed run with its reason", () => {
    const home = homeWith([
      { run: "g-1", parent: "local:home", name: "gang", gpu: true, host: "3 computers", state: "queued", members: 3 },
      { run: "f-1", parent: "local:home", name: "lost", gpu: false, host: "", state: "failed", members: 1, error: "its computer was lost" },
    ]);
    const nodes = jobNodes([parent], home);
    expect(nodes.map((n) => n.id)).toEqual(["job:g-1", "job:f-1"]);
    expect(nodes[0].job?.members).toBe(3);
    expect(nodes[1].job).toMatchObject({ state: "failed", error: "its computer was lost" });
  });
  it("shows nothing for a missing, broken or orphaned ledger", () => {
    expect(jobNodes([parent], path.join(os.tmpdir(), "no-such-home"))).toEqual([]);
    const bad = fs.mkdtempSync(path.join(os.tmpdir(), "jobrows-"));
    fs.mkdirSync(path.join(bad, "spawn"));
    fs.writeFileSync(path.join(bad, "spawn", "job-rows.json"), "{not json");
    expect(jobNodes([parent], bad)).toEqual([]);
    const orphan = homeWith([{ run: "p-9", parent: "local:gone", name: "x", gpu: false, host: "", state: "queued", members: 1 }]);
    expect(jobNodes([parent], orphan)).toEqual([]);
  });
});

describe("the live app's backend carries the child Spaces calls", () => {
  it("forwards the tree to the live backend instead of answering an empty one", async () => {
    const gate = new LiveGate<LiveServices>();
    const pending = new PendingSpacesBackend(gate);
    const live = { spaceTree: async () => [parent] } as unknown as SpacesBackend;
    pending.replace(live);
    expect(await pending.spaceTree()).toEqual([parent]);
  });
});
