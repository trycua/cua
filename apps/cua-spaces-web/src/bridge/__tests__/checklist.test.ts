// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, renderHook } from "@testing-library/react";
import { createElement, type ReactNode } from "react";
import { beforeEach, describe, expect, it } from "vitest";

import { createDemoAdapter } from "../adapters/demo";
import { BridgeProvider } from "../BridgeProvider";

import { CHECKLIST_STEPS, checklistDone, checklistView, readChecklist, resetChecklistCache, useChecklist } from "../checklist";
import type { DataAdapter } from "../adapter";
import type { HostEvent } from "../protocol";
import type { BridgeStore } from "../store";
import { sanitizeSignals, type TelemetrySignal } from "../ops/telemetry";
import { noCore, testCore, wasmBuilt } from "./testCore";

const KEY = "cua-spaces:checklist";

async function fakeStore(core?: BridgeStore["core"]): Promise<{ store: BridgeStore; sent: TelemetrySignal[] }> {
  const sent: TelemetrySignal[] = [];
  const store = {
    core: core ?? (await testCore()),
    telemetry: { track: async (s: readonly unknown[]) => void sent.push(...sanitizeSignals(s)) },
  } as unknown as BridgeStore;
  return { store, sent };
}

beforeEach(() => {
  localStorage.clear();
  resetChecklistCache();
});

describe("the getting-started checklist", () => {
  it.skipIf(!wasmBuilt)("keeps each step done between launches, in the stored shape", async () => {
    const { store } = await fakeStore();
    checklistDone(store, "create_space");
    resetChecklistCache();
    expect(readChecklist().done).toEqual({ create_space: true });
    expect(JSON.parse(localStorage.getItem(KEY)!)).toMatchObject({ done: { create_space: true }, dismissed: false });
  });

  it("reads only known steps and flags from storage", () => {
    localStorage.setItem(KEY, JSON.stringify({ done: { share: true, "me@example.com": true }, dismissed: "yes", shown: true }));
    expect(readChecklist()).toEqual({ done: { share: true }, dismissed: false, shown: true, skipped: {} });
    resetChecklistCache();
    localStorage.setItem(KEY, "{not json");
    expect(readChecklist()).toEqual({ done: {}, dismissed: false, shown: false, skipped: {} });
  });

  it.skipIf(!wasmBuilt)("records each step once, then completed with the last", async () => {
    const { store, sent } = await fakeStore();
    for (const step of CHECKLIST_STEPS) {
      checklistDone(store, step);
      checklistDone(store, step);
    }
    const steps = sent.map((s) => (s.type === "step" ? `${s.step} ${s.ok}` : s.type));
    expect(steps).toEqual([
      "checklist_add_machine true",
      "checklist_create_space true",
      "checklist_teleport true",
      "checklist_share true",
      "checklist_run_agent true",
      "checklist_completed true",
    ]);
  });

  it.skipIf(!wasmBuilt)("draws the core's rows and progress", async () => {
    const core = await testCore();
    const view = checklistView(core, { done: { teleport: true }, dismissed: false, shown: true, skipped: {} })!;
    expect(view).toMatchObject({ visible: true, complete: false, title: "Get started", progress: "1 of 5" });
    expect(view.rows.map((r) => [r.step, r.action, r.done])).toEqual([
      ["add_machine", "Add machine", false],
      ["create_space", "Create", false],
      ["teleport", "Teleport", true],
      ["share", "Share", false],
      ["run_agent", "Open Agents", false],
    ]);
    expect(checklistView(core, { done: {}, dismissed: true, shown: true, skipped: {} })?.visible).toBe(false);
  });

  it.skipIf(!wasmBuilt)("a skipped step counts as done and is kept, without an event of its own", async () => {
    const core = await testCore();
    const adapter = createDemoAdapter({ latencyMs: 0 });
    const { result } = renderHook(() => useChecklist(), { wrapper: ({ children }: { children: ReactNode }) => createElement(BridgeProvider, { adapter, core }, children) });
    act(() => result.current.skip("teleport"));
    expect(readChecklist().skipped).toEqual({ teleport: true });
    expect(JSON.parse(localStorage.getItem(KEY)!).skipped).toEqual({ teleport: true });
    resetChecklistCache();
    expect(readChecklist().skipped).toEqual({ teleport: true });
    const view = checklistView(core, readChecklist())!;
    expect(view.progress).toBe("1 of 5");
    expect(view.rows.find((r) => r.step === "teleport")).toMatchObject({ done: false, skipped: true });
    expect(view.hideLabel).toBe("Hide");
    adapter.dispose?.();
  });

  it("without the core: not drawn, and nothing changes", async () => {
    const { store, sent } = await fakeStore(noCore);
    checklistDone(store, "share");
    expect(readChecklist().done).toEqual({});
    expect(sent).toEqual([]);
    expect(checklistView(noCore, readChecklist())).toBeNull();
  });
});

/** A native host that keeps the checklist: answers `checklist.get` and
 * `checklist.send` from its own copy and says `checklist.changed`. */
function hostAdapter(initial: { done: string[]; dismissed: boolean; shown: boolean }) {
  let state = { ...initial, done: [...initial.done] };
  const sent: unknown[] = [];
  const listeners = new Set<(e: HostEvent) => void>();
  const order = CHECKLIST_STEPS as readonly string[];
  const adapter = {
    mode: "webkit",
    async call(op: string, args: { action?: { type: string; step?: string; state?: typeof state } }) {
      if (op === "checklist.get") return { state };
      if (op !== "checklist.send") throw new Error(`unexpected ${op}`);
      const a = args.action!;
      sent.push(a);
      if (a.type === "done") state = { ...state, done: order.filter((s) => s === a.step || state.done.includes(s)) };
      if (a.type === "merge") {
        const o = a.state!;
        state = { done: order.filter((s) => state.done.includes(s) || o.done.includes(s)), dismissed: state.dismissed || o.dismissed, shown: state.shown || o.shown };
      }
      return { state };
    },
    subscribe(l: (e: HostEvent) => void) {
      listeners.add(l);
      return () => listeners.delete(l);
    },
  } as unknown as DataAdapter;
  return {
    adapter,
    sent,
    change(next: typeof state) {
      state = next;
      for (const l of listeners) l({ type: "checklist.changed" });
    },
  };
}

const settle = () => new Promise((r) => setTimeout(r, 0));

describe("the host's copy (the Mac app and Electron)", () => {
  it("merges what this page kept before into the host's copy once, then forgets it", async () => {
    localStorage.setItem(KEY, JSON.stringify({ done: { teleport: true, add_machine: true }, dismissed: false, shown: true }));
    const host = hostAdapter({ done: ["add_machine", "create_space"], dismissed: false, shown: false });
    const { store, sent } = await fakeStore(noCore);
    (store as { adapter: DataAdapter }).adapter = host.adapter;
    // A step done goes to the host, which records its events.
    checklistDone(store, "share");
    await settle();
    await settle();
    expect(host.sent).toHaveLength(2);
    expect(host.sent).toContainEqual({ type: "merge", state: { done: ["add_machine", "teleport"], dismissed: false, shown: true, skipped: [] } });
    expect(host.sent).toContainEqual({ type: "done", step: "share" });
    expect(localStorage.getItem(KEY)).toBeNull();
    expect(sent).toEqual([]);
  });

  it("reads the host's copy again when it changed", async () => {
    const host = hostAdapter({ done: [], dismissed: false, shown: false });
    const { store } = await fakeStore(noCore);
    (store as { adapter: DataAdapter }).adapter = host.adapter;
    checklistDone(store, "teleport");
    await settle();
    host.change({ done: ["add_machine", "teleport"], dismissed: true, shown: true });
    await settle();
    expect(host.sent).toEqual([{ type: "done", step: "teleport" }]);
  });
});
