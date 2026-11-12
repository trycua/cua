// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Home's pool: the app core adds the computers up and places a job on the one with the most free room
// (`pool.summary`, `pool.place`); this file's helpers only feed it and say the result.

import { beforeAll, describe, expect, it } from "vitest";

import type { AgentChat, Machine, Space } from "@/bridge";
import type { CoreClient } from "@/bridge/core";
import { noCore, testCore, wasmBuilt } from "@/bridge/__tests__/testCore";

import {
  computerSpaceIds,
  computerSpec,
  freeText,
  jobsByComputer,
  placeJob,
  poolHeadline,
  poolLine,
  poolSummary,
  reasonText,
  whereOn,
  type PoolComputer,
} from "./pool";
import type { TreeNode } from "./spaces-tree";

const GB = 1024 ** 3;

const pc = (id: string, cores: number, busy: number, memGb: number, usedGb: number, extra: Partial<PoolComputer> = {}): PoolComputer => ({
  id,
  current: false,
  online: true,
  notSharing: false,
  resources: { cpuCount: cores, cpuPercent: busy, memoryTotal: memGb * GB, memoryUsed: usedGb * GB },
  jobs: 0,
  ...extra,
});

/** The decision page's three computers: Studio Mac, this MacBook Pro and an offline Linux box. */
const three = () => [pc("mbp", 12, 25, 24, 14, { current: true }), pc("linux", 8, 0, 8, 1, { online: false }), pc("studio", 20, 40, 64, 24)];

describe.skipIf(!wasmBuilt)("pool, in the app core", () => {
  let core: CoreClient;
  beforeAll(async () => {
    core = await testCore();
  });

  it("adds every computer up and counts free room only where a job can run", () => {
    const s = poolSummary(core, three());
    expect(s).toEqual({ computers: 3, online: 2, cores: 40, memory: 96 * GB, freeCores: 21, freeMemory: 50 * GB, unknown: 0 });
    expect(poolHeadline(s)).toEqual(["3 computers", "40 cores", "96 GB"]);
    expect(poolLine(s)).toBe("2 online now, with 21 cores and 50 GB free for new jobs.");
  });

  it("only This Mac: one computer, still a pool", () => {
    const s = poolSummary(core, [pc("me", 12, 50, 64, 32, { current: true })]);
    expect(poolHeadline(s)).toEqual(["1 computer", "12 cores", "64 GB"]);
    expect(poolLine(s)).toBe("1 online now, with 6 cores and 32 GB free for new jobs.");
    expect(placeJob(core, { computers: [pc("me", 12, 50, 64, 32, { current: true })] }).chosen).toBe("me");
  });

  it("places a job on the most free room, this computer last, offline never", () => {
    const p = placeJob(core, { computers: three() });
    expect(p.chosen).toBe("studio");
    expect(p.rows.map((r) => [r.id, r.reason])).toEqual([
      ["studio", "best-fit"],
      ["mbp", "you-are-here"],
      ["linux", "offline"],
    ]);
    expect(reasonText(p.rows[0]!)).toBe("12 cores free, best fit");
    expect(reasonText(p.rows[1]!)).toBe("You’re working here");
    expect(reasonText(p.rows[2]!)).toBe("Offline, can’t take jobs now");
  });

  it("moves a job off a picked computer that can't take it, and off one that failed to start it", () => {
    expect(placeJob(core, { computers: three(), picked: "linux" })).toMatchObject({ chosen: "studio", movedFrom: "linux" });
    expect(placeJob(core, { computers: three(), picked: "studio", exclude: ["studio"] })).toMatchObject({ chosen: "mbp", movedFrom: "studio" });
    expect(placeJob(core, { computers: [pc("a", 4, 0, 8, 0, { online: false })], picked: "a" })).toMatchObject({ chosen: null, movedFrom: null });
  });

  it("a computer whose numbers are unknown is still offered, after the measured ones", () => {
    const p = placeJob(core, { computers: [{ ...pc("old", 0, 0, 0, 0), resources: null }, pc("new", 4, 75, 8, 6)] });
    expect(p.chosen).toBe("new");
    expect(p.rows[1]).toMatchObject({ id: "old", eligible: true, reason: "unknown" });
    expect(reasonText(p.rows[1]!)).toBe("Free room not known yet");
  });
});

describe("pool without the core", () => {
  it("adds nothing up and keeps the pick, else the first computer that can take a job", () => {
    const s = poolSummary(noCore, three());
    expect(s).toMatchObject({ computers: 3, online: 2, cores: 0, unknown: 3 });
    expect(poolHeadline(s)).toEqual(["3 computers"]);
    expect(placeJob(noCore, { computers: three() }).chosen).toBe("mbp");
    expect(placeJob(noCore, { computers: three(), picked: "linux" })).toMatchObject({ chosen: "mbp", movedFrom: "linux" });
  });
});

describe("pool words and places", () => {
  it("says a computer's size and its free room", () => {
    const r = { cpuCount: 20, cpuPercent: 40, memoryTotal: 64 * GB, memoryUsed: 24 * GB };
    expect(computerSpec("Mac Studio", r)).toEqual(["Mac Studio", "20 cores", "64 GB"]);
    expect(computerSpec("ThinkCentre", null)).toEqual(["ThinkCentre"]);
    expect(freeText({ cores: 12, memory: 40 * GB, fraction: 0.6 }, r)).toEqual(["12 of 20 cores", "40 GB"]);
    expect(freeText(null, r)).toEqual([]);
  });

  const node = (space: Partial<Space> | null, children: string[] = []): TreeNode =>
    ({
      space: space as Space | null,
      online: true,
      children: children.map((id) => ({ space: { id } })),
    }) as unknown as TreeNode;
  const machine = (id: string, extra: Partial<Machine> = {}): Machine => ({ id, name: id, current: false, ...extra }) as Machine;

  it("counts an agent as a computer's job by the Space it works in", () => {
    const here = {
      machine: machine("me", { current: true, relayId: "r-me" }),
      ids: computerSpaceIds(node({ id: "this-mac" }), machine("me", { current: true, relayId: "r-me" })),
    };
    const mini = { machine: machine("mini"), ids: computerSpaceIds(node({ id: "relay:mini" }, ["relay:mini/qa"]), machine("mini")) };
    const chat = (key: string, spaceId: string, tone: AgentChat["tone"]) => ({ key, tone, places: [{ spaceId, spaceName: spaceId }] }) as AgentChat;
    const jobs = jobsByComputer(
      [chat("a", "relay:mini/qa", "working"), chat("b", "relay:r-me", "you"), chat("c", "relay:mini", "asleep"), chat("d", "elsewhere", "working")],
      [here, mini],
    );
    expect(jobs.get("mini")!.map((c) => c.key)).toEqual(["a"]);
    expect(jobs.get("me")!.map((c) => c.key)).toEqual(["b"]);
  });

  it("runs on this computer itself, another's shared desktop, or a new Linux Space made on it", () => {
    expect(whereOn({ node: node({ id: "this-mac" }), machine: machine("me", { current: true }) })).toEqual({ kind: "this-mac" });
    expect(whereOn({ node: node({ id: "relay:mini", name: "Mac mini" }), machine: machine("mini") })).toMatchObject({
      kind: "space",
      space: { id: "relay:mini" },
    });
    expect(whereOn({ node: node(null), machine: machine("box", { name: "Linux box" }) })).toEqual({
      kind: "new",
      os: "linux",
      on: { machine: "box", name: "Linux box" },
    });
  });
});
