// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { beforeEach, describe, expect, it } from "vitest";
import type { DataAdapter } from "../adapter";
import { createDemoAdapter } from "../adapters/demo";
import type { NotificationsData } from "../settings-feature";
import type { HostEvent } from "../protocol";
import type { Space } from "../contracts/spaces";
import { BridgeStore } from "../store";
import { testCore, until, wasmBuilt } from "./testCore";

/** The demo host whose `spaces.create` never answers while `stuck` (the
 * create never reaches the daemon), and whose events a test can push. */
function stuckHost() {
  const demo = createDemoAdapter({ latencyMs: 0 });
  const listeners = new Set<(e: HostEvent) => void>();
  const host = {
    stuck: true,
    creates: 0,
    emit: (e: HostEvent) => listeners.forEach((l) => l(e)),
  };
  const adapter: DataAdapter = Object.assign(Object.create(demo), {
    call: ((method: string, args: unknown) => {
      if (method === "spaces.create") {
        host.creates++;
        if (host.stuck) return new Promise(() => {});
      }
      return demo.call(method as never, args as never);
    }) as DataAdapter["call"],
    subscribe: (l: (e: HostEvent) => void) => {
      listeners.add(l);
      const off = demo.subscribe(l);
      return () => {
        listeners.delete(l);
        off();
      };
    },
  });
  return { adapter, host, demo };
}

const errors = (store: BridgeStore) =>
  (store.extras.get<NotificationsData>("notifications").data?.view?.rows ?? []).filter((r) => r.text.startsWith("Couldn't create"));

describe.skipIf(!wasmBuilt)("a create the host never answers", () => {
  // This app's own notifications persist in localStorage.
  beforeEach(() => globalThis.localStorage?.clear());
  it("fails on the stall with one notification, and Try again runs it again", async () => {
    let now = 1_800_000_000_000;
    const { adapter, host } = stuckHost();
    const store = new BridgeStore(adapter, await testCore(), { now: () => now, tickMs: 5 });
    store.ensure("spaces");
    store.extras.ensure("notifications");
    await until(() => expect(store.get<Space[]>("spaces").data).toBeTruthy());
    await until(() => expect(store.extras.get<NotificationsData>("notifications").data).toBeTruthy());

    void store.createSpace({ image: "ubuntu-xfce", name: "e2e-1005-linux", os: "linux" });
    const row = () => store.get<Space[]>("spaces").data?.find((s) => s.name === "e2e-1005-linux");
    await until(() => expect(row()?.id).toMatch(/^pending:/));
    const pendingId = row()!.id;
    expect(store.canRetryCreate(pendingId)).toBe(true);

    // No progress for minutes: the core stalls the row (the host's call
    // never ends, so nothing else would say it failed).
    now += 5 * 60_000;
    await until(() => expect(row()?.progress?.error).toBeTruthy());
    await until(() => expect(errors(store)).toHaveLength(1));
    expect(errors(store)[0]!.text).toContain("Couldn't create e2e-1005-linux");
    // More ticks never post it again.
    now += 60_000;
    await new Promise((r) => setTimeout(r, 40));
    expect(errors(store)).toHaveLength(1);

    // Try again: a new row for the same request; the failed one goes.
    host.stuck = false;
    const next = store.retryCreate(pendingId);
    expect(next).not.toBe(pendingId);
    expect(host.creates).toBe(2);
    await until(() => expect(store.get<Space[]>("spaces").data?.some((s) => s.id === pendingId)).toBe(false));
    await until(() => expect(store.createdId(next)).toBeTruthy(), 5000);
    expect(errors(store)).toHaveLength(1);
  });

  it("a create the host rejects is noted once too", async () => {
    const { adapter } = stuckHost();
    const demoCall = adapter.call.bind(adapter);
    adapter.call = ((method: string, args: never) =>
      method === "spaces.create" ? Promise.reject(new Error("Cua's background service didn't start this create within 30 s.")) : demoCall(method as never, args)) as DataAdapter["call"];
    const store = new BridgeStore(adapter, await testCore(), { tickMs: 5 });
    store.ensure("spaces");
    store.extras.ensure("notifications");
    await until(() => expect(store.extras.get<NotificationsData>("notifications").data).toBeTruthy());
    await expect(store.createSpace({ image: "ubuntu-xfce", name: "boom", os: "linux" })).rejects.toThrow(/didn't start/);
    await until(() => expect(errors(store)).toHaveLength(1));
    await new Promise((r) => setTimeout(r, 40));
    expect(errors(store)).toHaveLength(1);
  });

  it("reads the list again once the host's startup is ready", async () => {
    const { adapter, host } = stuckHost();
    const store = new BridgeStore(adapter, await testCore(), { tickMs: 5 });
    let lists = 0;
    const call = adapter.call.bind(adapter);
    adapter.call = ((method: string, args: never) => {
      if (method === "spaces.list") lists++;
      return call(method as never, args);
    }) as DataAdapter["call"];
    store.ensure("spaces");
    await until(() => expect(lists).toBe(1));
    host.emit({ type: "startup.changed", state: { phase: "starting", slow: false, title: "Starting Cua…", body: "", actions: [] } });
    await new Promise((r) => setTimeout(r, 20));
    expect(lists).toBe(1);
    host.emit({ type: "startup.changed", state: { phase: "ready", slow: false, title: "", body: "", actions: [] } });
    await until(() => expect(lists).toBe(2));
  });
});
