// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { beforeEach, describe, expect, it } from "vitest";
import type { DataAdapter } from "../adapter";
import { createDemoAdapter, DEMO_CREATE_FAILURES, type DemoOptions } from "../adapters/demo";
import { HOST_RUNS_CREATES, loadCreateStreaks } from "../create-retry";
import type { HostEvent } from "../protocol";
import type { Space } from "../contracts/spaces";
import type { TelemetrySignal } from "../ops/telemetry";
import { BridgeStore } from "../store";
import { testCore, until, wasmBuilt } from "./testCore";

/** The demo host, counting `spaces.create` calls and the usage signals the
 * page sends; `mode` stands in for a native host. */
function host(options: DemoOptions, mode?: DataAdapter["mode"]) {
  const demo = createDemoAdapter({ latencyMs: 0, stepMs: 4, ...options });
  const seen = { creates: 0, signals: [] as TelemetrySignal[] };
  const adapter: DataAdapter = Object.assign(Object.create(demo), {
    ...(mode ? { mode } : {}),
    call: ((method: string, args: unknown) => {
      if (method === "spaces.create") seen.creates++;
      if (method === "telemetry.track") seen.signals.push(...((args as { signals: TelemetrySignal[] }).signals ?? []));
      if (method === "settings.get") {
        return Promise.resolve(demo.call(method as never, args as never)).then((s: { telemetry?: object }) => ({
          ...s,
          telemetry: { ...s.telemetry, enabled: true, noticeShown: true },
        }));
      }
      return demo.call(method as never, args as never);
    }) as DataAdapter["call"],
    subscribe: (l: (e: HostEvent) => void) => demo.subscribe(l),
  });
  return { adapter, seen };
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

describe.skipIf(!wasmBuilt)("a create that fails", () => {
  beforeEach(() => globalThis.localStorage?.clear());

  it("runs once more by itself after a passing failure, saying Retrying…, and counts once", async () => {
    const { adapter, seen } = host({ failCreates: "runtime-once" });
    const store = await ready(adapter);
    const labels = new Set<string>();
    const off = store.subscribe(() => {
      const label = row(store, "box")?.progress?.label;
      if (label) labels.add(label);
    });
    const space = await store.createSpace({ image: "ubuntu-xfce", name: "box", os: "linux" });
    off();
    expect(space.name).toBe("box");
    // Two calls to the host, one visible retry, no failure shown.
    expect(seen.creates).toBe(2);
    expect(labels.has("Retrying…")).toBe(true);
    expect(labels.has("Failed")).toBe(false);
    // One final outcome: ok, retried once, past the runtime's start.
    await until(() => expect(seen.signals.some((s) => s.type === "space-create")).toBe(true));
    const created = seen.signals.filter((s) => s.type === "space-create");
    expect(created).toHaveLength(1);
    expect(created[0]).toMatchObject({ outcome: "ok", retryAttempt: 1, errorSubclass: "runtime_start" });
  });

  it("is never retried for a lasting cause, and offers the fix the second time", async () => {
    const { adapter, seen } = host({ failCreates: "disk" });
    const store = await ready(adapter);
    await expect(store.createSpace({ image: "macos", name: "mac", os: "macos" })).rejects.toThrow(/disk space/);
    expect(seen.creates).toBe(1);
    const first = row(store, "mac")!;
    expect(first.progress?.error).toMatch(/Not enough disk space/);
    expect(first.progress?.fix).toBeUndefined();
    // Gone from the list, still counted (and kept for the next launch).
    await store.dismissCreate(first.id);
    expect(loadCreateStreaks()).toEqual([expect.objectContaining({ kind: "insufficient_disk", count: 1 })]);
    // The same failure again: the fix, not a plain Try again.
    await expect(store.createSpace({ image: "macos", name: "mac", os: "macos" })).rejects.toThrow(/disk space/);
    expect(seen.creates).toBe(2);
    const second = row(store, "mac")!;
    expect(second.progress?.fix).toMatchObject({ action: "copy-command", value: "cua cache prune", label: "Copy cua cache prune" });
    // Each failure counted once, never retried.
    const created = seen.signals.filter((s) => s.type === "space-create");
    expect(created.map((s) => (s as { retryAttempt?: number }).retryAttempt)).toEqual([0, 0]);
    // The kept counts never hold the error's text.
    expect(JSON.stringify(loadCreateStreaks())).not.toContain("/Users/me");
  });

  it("follows a native host's own retry and leaves its usage events to it", async () => {
    expect(HOST_RUNS_CREATES.webkit && HOST_RUNS_CREATES.electron).toBe(true);
    const demo = createDemoAdapter({ latencyMs: 0, stepMs: 4 });
    const listeners = new Set<(e: HostEvent) => void>();
    const seen = { creates: 0, tracked: 0 };
    let release!: () => void;
    const adapter: DataAdapter = Object.assign(Object.create(demo), {
      mode: "webkit",
      call: ((method: string, args: { pendingId?: string }) => {
        if (method === "telemetry.track") seen.tracked++;
        if (method === "spaces.create") {
          seen.creates++;
          // The host fails once, says it runs it again, then finishes.
          const pendingId = args.pendingId!;
          listeners.forEach((l) =>
            l({ type: "spaces.createProgress", progress: { pendingId, phase: "preparing", detail: "", retry: { error: DEMO_CREATE_FAILURES.runtime, errorVariant: "Runtime" } } }),
          );
          return new Promise((r) => (release = () => r(demo.call(method as never, args as never))));
        }
        return demo.call(method as never, args as never);
      }) as DataAdapter["call"],
      subscribe: (l: (e: HostEvent) => void) => {
        listeners.add(l);
        return () => listeners.delete(l);
      },
    });
    const store = await ready(adapter);
    const done = store.createSpace({ image: "ubuntu-xfce", name: "box", os: "linux" });
    await until(() => expect(row(store, "box")?.progress?.label).toBe("Retrying…"));
    expect(row(store, "box")?.progress?.retrying).toBe(true);
    release();
    await done;
    // The page never runs it again itself, and sends no create events.
    expect(seen.creates).toBe(1);
    expect(seen.tracked).toBe(0);
  });
});
