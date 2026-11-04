// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Rows shown on purpose (parity, previews: `showMachines`) stay shown when a
// host read that started before them answers afterwards.

import { describe, expect, it } from "vitest";
import type { DataAdapter } from "../adapter";
import { createDemoAdapter } from "../adapters/demo";
import type { MachineRow } from "../contracts/host";
import { BridgeStore } from "../store";
import { testCore, until, wasmBuilt } from "./testCore";

describe.skipIf(!wasmBuilt)("pinned machine rows", () => {
  it("a machines.list that answers after showMachines doesn't replace them", async () => {
    const demo = createDemoAdapter({ latencyMs: 0 });
    let answer: (() => void) | null = null;
    const adapter: DataAdapter = Object.assign(Object.create(demo), {
      call: ((method: string, args: unknown) => {
        if (method === "machines.list") {
          return new Promise((resolve) => {
            answer = () => resolve(demo.call("machines.list", {}));
          });
        }
        return demo.call(method as never, args as never);
      }) as DataAdapter["call"],
    });
    const store = new BridgeStore(adapter, await testCore());
    store.ensure("machines");
    await until(() => answer !== null);
    const pinned: MachineRow[] = [{ id: "pinned", name: "Pinned Mac", via: "local", online: true, os: "macos", current: true, limits: [] } as MachineRow];
    store.showMachines(pinned, 1);
    answer!();
    await new Promise((r) => setTimeout(r, 20));
    const ids = (store.get<{ id: string }[]>("machines").data ?? []).map((m) => m.id);
    expect(ids).toContain("pinned");
    expect(ids).not.toContain("mac-mini");
  });
});
