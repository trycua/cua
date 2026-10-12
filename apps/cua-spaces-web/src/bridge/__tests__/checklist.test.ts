// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { beforeEach, describe, expect, it } from "vitest";

import { CHECKLIST_STEPS, checklistDone, checklistView, readChecklist, resetChecklistCache } from "../checklist";
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
    expect(readChecklist()).toEqual({ done: { share: true }, dismissed: false, shown: true });
    resetChecklistCache();
    localStorage.setItem(KEY, "{not json");
    expect(readChecklist()).toEqual({ done: {}, dismissed: false, shown: false });
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
    const view = checklistView(core, { done: { teleport: true }, dismissed: false, shown: true })!;
    expect(view).toMatchObject({ visible: true, complete: false, title: "Get started", progress: "1 of 5" });
    expect(view.rows.map((r) => [r.step, r.action, r.done])).toEqual([
      ["add_machine", "Add machine", false],
      ["create_space", "Create", false],
      ["teleport", "Teleport", true],
      ["share", "Share", false],
      ["run_agent", "Open Agents", false],
    ]);
    expect(checklistView(core, { done: {}, dismissed: true, shown: true })?.visible).toBe(false);
  });

  it("without the core: not drawn, and nothing changes", async () => {
    const { store, sent } = await fakeStore(noCore);
    checklistDone(store, "share");
    expect(readChecklist().done).toEqual({});
    expect(sent).toEqual([]);
    expect(checklistView(noCore, readChecklist())).toBeNull();
  });
});
