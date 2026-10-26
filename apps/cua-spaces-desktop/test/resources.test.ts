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

  it("warns as the daemon does", () => {
    const GB = 1e9;
    expect(diskWarning(90 * GB, 1000 * GB).level).toBe("low");
    expect(diskWarning(120 * GB, 1000 * GB).level).toBe("ok");
    expect(diskWarning(45 * GB, 256 * GB).level).toBe("low");
    expect(diskWarning(4 * GB, 1000 * GB).level).toBe("critical");
  });
});
