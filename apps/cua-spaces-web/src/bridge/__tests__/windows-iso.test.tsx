// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { beforeEach, describe, expect, it } from "vitest";
import type { DataAdapter } from "../adapter";
import { createDemoAdapter } from "../adapters/demo";
import { runCreateFix } from "../create-retry";
import type { HostEvent } from "../protocol";
import type { Space } from "../contracts/spaces";
import { BridgeStore } from "../store";
import { testCore, until, wasmBuilt } from "./testCore";

/** The demo host where Microsoft refuses the Windows download until a disc
 * image is chosen, recording the calls the page makes. */
function host(chosen: { name: string } | null | Error = { name: "Win11_ARM64.iso" }) {
  const demo = createDemoAdapter({ latencyMs: 0, stepMs: 4, failCreates: "windows-limited" });
  const calls: [string, unknown][] = [];
  const adapter: DataAdapter = Object.assign(Object.create(demo), {
    call: ((method: string, args: unknown) => {
      calls.push([method, args]);
      if (method === "windows.chooseIso") {
        if (chosen instanceof Error) return Promise.reject(chosen);
        if (chosen) return demo.call(method as never, args as never);
        return Promise.resolve(null);
      }
      return demo.call(method as never, args as never);
    }) as DataAdapter["call"],
    subscribe: (l: (e: HostEvent) => void) => demo.subscribe(l),
  });
  return { adapter, calls };
}

async function ready(adapter: DataAdapter) {
  let now = 1_800_000_000_000;
  const store = new BridgeStore(adapter, await testCore(), { now: () => (now += 10), tickMs: 5 });
  store.ensure("spaces");
  store.ensure("settings");
  await until(() => expect(store.get<Space[]>("spaces").data).toBeTruthy());
  return store;
}

const row = (store: BridgeStore, name: string) => store.get<Space[]>("spaces").data?.find((s) => s.name === name);

describe.skipIf(!wasmBuilt)("Choose Windows ISO…", () => {
  beforeEach(() => globalThis.localStorage?.clear());

  it("is offered on the first Sentinel failure, beside Microsoft's page and Try again", async () => {
    const { adapter } = host();
    const store = await ready(adapter);
    await expect(store.createSpace({ image: "ghcr.io/trycua/windows:11", name: "win", os: "windows" })).rejects.toThrow(/Microsoft is limiting/);
    const failed = row(store, "win")!;
    expect(failed.progress?.error).not.toMatch(/Sentinel|--iso/);
    expect(failed.progress?.fix).toMatchObject({
      action: "choose-windows-iso",
      label: "Choose Windows ISO…",
      linkLabel: "Open Microsoft's download page",
      linkUrl: "https://www.microsoft.com/software-download/windows11arm64",
      keepRetry: true,
    });
  });

  it("registers the chosen file for the failed create's image and runs the create again with it", async () => {
    const { adapter, calls } = host();
    const store = await ready(adapter);
    await expect(store.createSpace({ image: "ghcr.io/trycua/windows:11", name: "win", os: "windows" })).rejects.toThrow();
    const failed = row(store, "win")!;
    const chosen = await store.chooseWindowsIsoAndRetry(failed.id);
    expect(chosen?.name).toBe("Win11_ARM64.iso");
    expect(calls).toContainEqual(["windows.chooseIso", { image: "ghcr.io/trycua/windows:11" }]);
    // The same create, a new row, and this time it goes through.
    await until(() => expect(row(store, "win")?.progress?.fix).toBeUndefined());
    await until(() => expect(row(store, "win")?.status).toBe("running"));
  });

  it("does nothing when the picker is cancelled, and says why a wrong file is refused", async () => {
    const cancelled = host(null);
    const a = await ready(cancelled.adapter);
    await expect(a.createSpace({ image: "ghcr.io/trycua/windows:11", name: "win", os: "windows" })).rejects.toThrow();
    expect(await a.chooseWindowsIsoAndRetry(row(a, "win")!.id)).toBeNull();
    expect(cancelled.calls.filter(([m]) => m === "spaces.create")).toHaveLength(1);

    const wrong = host(new Error("notes.txt is not a Windows disc image (.iso). Choose the file you downloaded from Microsoft."));
    const b = await ready(wrong.adapter);
    await expect(b.chooseWindowsIso()).rejects.toThrow(/not a Windows disc image/);
  });

  it("is a fix button the page runs, with the link the core gave", async () => {
    const opened: string[] = [];
    let picked = 0;
    await runCreateFix(
      { text: "t", label: "Choose Windows ISO…", action: "choose-windows-iso", keepRetry: true },
      { copy: () => {}, openExternal: (u) => void opened.push(u), details: () => {}, chooseWindowsIso: () => void picked++ },
    );
    expect(picked).toBe(1);
    expect(opened).toEqual([]);
  });
});
