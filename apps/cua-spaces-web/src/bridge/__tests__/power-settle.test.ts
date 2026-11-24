// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";
import type { DataAdapter } from "../adapter";
import { createDemoAdapter } from "../adapters/demo";
import type { Space } from "../contracts/spaces";
import { BridgeStore } from "../store";
import { testCore, until, wasmBuilt } from "./testCore";

/** The demo host without its `spaces.changed` after a power change, and no polling:
 * what the Electron shell does (cua keeps the power state in its daemon, not in
 * spaces.json, so the registry watcher says nothing). */
function quietHost(): DataAdapter {
  const demo = createDemoAdapter({ latencyMs: 0 });
  return Object.assign(Object.create(demo), {
    subscribe: (l: Parameters<DataAdapter["subscribe"]>[0]) => demo.subscribe((e) => (e.type === "spaces.changed" ? undefined : l(e))),
  }) as DataAdapter;
}

describe.skipIf(!wasmBuilt)("a power change on a host that sends no registry event", () => {
  it("clears Suspending… and Starting… once the registry shows the Space off or on", async () => {
    const store = new BridgeStore(quietHost(), await testCore(), { tickMs: 5 });
    store.ensure("spaces");
    const row = () => store.get<Space[]>("spaces").data?.find((s) => s.id === "local:design-review");
    await until(() => expect(row()?.power).toBeTruthy());
    expect(row()?.power?.off).toBe(false);

    await store.setPower("local:design-review", false);
    await until(() => expect(row()?.power?.off).toBe(true));
    expect(row()?.power?.turningOn).toBeUndefined();

    await store.setPower("local:design-review", true);
    await until(() => expect(row()?.power?.off).toBe(false));
    expect(row()?.power?.turningOn).toBeUndefined();
  });
});
