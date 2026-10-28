// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// `resources.get`: this computer's numbers in the page's shape, checked
// against the web contract; busy from the second read on.
import { describe, expect, it } from "vitest";
import { OP_SHAPES, SHAPE_DEFS } from "../../cua-spaces-web/src/bridge/contracts/shapes";
import { validate } from "../../cua-spaces-web/src/bridge/contracts/schema";
import type { BridgeContext } from "../src/bridge/context";
import { diskWarning, resourcesMethods } from "../src/bridge/resources";

describe("resources.get", () => {
  it("answers this computer's numbers in the contract's shape, busy from the second read", async () => {
    let n = 0;
    const ticks = () => (n++ === 0 ? { busy: 100, total: 1000 } : { busy: 400, total: 2000 });
    const get = resourcesMethods({} as BridgeContext, { ticks, now: () => 42, home: () => process.cwd() })["resources.get"]!;
    const first = (await get({}, {})) as { thisMac: { cpuPercent: number; diskTotal: number; sampledAtMs: number }; spaces: unknown[] };
    expect(validate(OP_SHAPES["resources.get"], first, SHAPE_DEFS)).toEqual([]);
    expect(first.thisMac.cpuPercent).toBe(0);
    expect(first.thisMac.diskTotal).toBeGreaterThan(0);
    expect(first.thisMac.sampledAtMs).toBe(42);
    expect(first.spaces).toEqual([]);
    const second = (await get({}, {})) as typeof first;
    expect(second.thisMac.cpuPercent).toBe(30);
  });

  it("answers the SDK's snapshot when it has one: every Space, here and on your other computers", async () => {
    const GB = 1_000_000_000n;
    const warning = { level: "low", free: 41n * GB, total: 1000n * GB, threshold: 100n * GB };
    const computer = (id: string, name: string) => ({
      id, name, online: true, sampledAtMs: 7n, cpuCount: 12, cpuPercent: 30.5, loadAverage1m: 2.5,
      memoryTotal: 64n * GB, memoryUsed: 41n * GB, diskTotal: 1000n * GB, diskFree: 41n * GB,
      spacesCount: 1, spacesRunning: 1, spacesCpuAllocated: 4, spacesMemoryAllocated: 8n * GB, spacesMemoryUsed: 3n * GB,
      spacesDiskBytes: 19n * GB, spacesDiskExact: true, warning,
    });
    const sdk = {
      sequence: 9n,
      sampledAtMs: 7n,
      thisMac: computer("this-mac", ""),
      spaces: [
        {
          id: "local:design-review", powerState: "running", cpuAllocated: 4, memoryAllocated: 8n * GB, diskAllocated: 80n * GB,
          usage: { cpuCount: 4, cpuPercent: 22, loadAverage1m: 1, memoryUsed: 3n * GB, memoryTotal: 8n * GB, memoryLimited: true, diskUsed: 30n * GB, diskTotal: 80n * GB, diskLimited: true },
          diskOnHost: { bytes: 19n * GB, allocated: 34n * GB, sharesSpace: true },
        },
        // One of yours on the Mac mini: only its size there.
        { id: "relay:space-qa-1234", powerState: "stopped", cpuAllocated: 0, memoryAllocated: 0n, diskAllocated: 0n, usage: undefined, diskOnHost: { bytes: 21n * GB, allocated: 21n * GB, sharesSpace: false } },
        // Measured only as an upper bound.
        { id: "local:clone", powerState: "stopped", cpuAllocated: 2, memoryAllocated: 0n, diskAllocated: 0n, usage: undefined, diskOnHost: { bytes: undefined, allocated: 30n * GB, sharesSpace: true } },
      ],
      computers: [computer("mac-mini", "Mac mini")],
      warning,
    };
    const asked: boolean[] = [];
    const ctx = { model: { backend: { resources: async (visible: boolean) => (asked.push(visible), sdk) } } } as unknown as BridgeContext;
    const get = resourcesMethods(ctx, { ticks: () => ({ busy: 0, total: 0 }), now: () => 42, home: () => process.cwd() })["resources.get"]!;
    const snap = (await get({ visible: false }, {})) as { sequence: number; thisMac: { warning: { level: string } }; spaces: { id: string; usage: unknown; diskOnHost: { bytes: number | null } | null }[]; computers: { id: string }[] };
    expect(asked).toEqual([false]);
    expect(validate(OP_SHAPES["resources.get"], snap, SHAPE_DEFS)).toEqual([]);
    expect(snap.sequence).toBe(9);
    expect(snap.thisMac.warning.level).toBe("low");
    expect(snap.spaces.map((s) => [s.id, s.diskOnHost?.bytes])).toEqual([
      ["local:design-review", 19e9],
      ["relay:space-qa-1234", 21e9],
      ["local:clone", null],
    ]);
    expect(snap.spaces[1]!.usage).toBeNull();
    expect(snap.computers.map((c) => c.id)).toEqual(["mac-mini"]);
  });

  it("answers this computer's own numbers when the SDK can't", async () => {
    const ctx = { model: { backend: { resources: async () => { throw new Error("daemon down"); } } } } as unknown as BridgeContext;
    const get = resourcesMethods(ctx, { ticks: () => ({ busy: 0, total: 0 }), now: () => 5, home: () => process.cwd() })["resources.get"]!;
    const snap = (await get({ visible: true }, {})) as { thisMac: { sampledAtMs: number }; spaces: unknown[] };
    expect(validate(OP_SHAPES["resources.get"], snap, SHAPE_DEFS)).toEqual([]);
    expect(snap.thisMac.sampledAtMs).toBe(5);
    expect(snap.spaces).toEqual([]);
  });

  it("warns as the daemon does", () => {
    const GB = 1e9;
    expect(diskWarning(90 * GB, 1000 * GB).level).toBe("low");
    expect(diskWarning(120 * GB, 1000 * GB).level).toBe("ok");
    expect(diskWarning(45 * GB, 256 * GB).level).toBe("low");
    expect(diskWarning(4 * GB, 1000 * GB).level).toBe("critical");
  });
});
