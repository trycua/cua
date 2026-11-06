// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";
import type { DataAdapter } from "../adapter";
import { createDemoAdapter } from "../adapters/demo";
import type { Space } from "../contracts/spaces";
import { BridgeStore } from "../store";
import { testCore, until, wasmBuilt } from "./testCore";

// An agent home (CUA-2630) reaches the host as a plain `spaces.create` whose
// config carries children and the `cua.home.*` labels untouched:
// the hosts (Electron, SwiftUI, Tauri) forward them to the SDK.
describe.skipIf(!wasmBuilt)("an agent home's create", () => {
  it("passes children and labels to the host as they are", async () => {
    const demo = createDemoAdapter({ latencyMs: 0, stepMs: 4 });
    const configs: Record<string, unknown>[] = [];
    const adapter: DataAdapter = Object.assign(Object.create(demo), {
      call: ((method: string, args: unknown) => {
        if (method === "spaces.create") configs.push((args as { config: Record<string, unknown> }).config);
        return demo.call(method as never, args as never);
      }) as DataAdapter["call"],
      subscribe: demo.subscribe.bind(demo),
    });
    const store = new BridgeStore(adapter, await testCore(), { tickMs: 5 });
    store.ensure("spaces");
    await until(() => expect(store.get<Space[]>("spaces").data).toBeTruthy());
    const labels = { "cua.home": "1", "cua.home.agent": "claude-code", "cua.home.list": "all" };
    // A published image: the core refuses one the pipeline has not published yet
    // (the slim tag is published).
    const image = "ghcr.io/trycua/linux:24.04-slim";
    await store.createSpace({ image, name: "home-x", os: "linux", children: true, labels });
    expect(configs).toHaveLength(1);
    expect(configs[0]).toMatchObject({ image, children: true, labels });
  });
});
