// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The getting-started checklist's one copy, kept by the host app
 * (`checklist.json` beside its settings), so the classic Mac UI, the New
 * UI and the Electron app count the same.
 *
 * | Operation | Electron | SwiftUI | Tauri |
 * |---|---|---|---|
 * | `checklist.get` | the host method `checklist.get` | the host method `checklist.get` | unsupported |
 * | `checklist.send` | the host method `checklist.send` | the host method `checklist.send` | unsupported |
 *
 * `checklist.send` takes a step done or skipped, `shown`, `dismiss`, `reopen`, or
 * `merge` (progress this page kept itself before, merged once). The host
 * runs the core's `checklist.reduce`, saves it and records its usage
 * events; it ticks steps off from what it shows itself
 * (`checklist::observe_from`), so the page never sends `observe` there.
 * The host says `checklist.changed` when the copy changed. Where
 * unsupported, the page keeps its own copy (`bridge/checklist.ts`).
 */

import { UnsupportedOperationError, type BridgeMode } from "../adapter";
import type { OpCoverage } from "../coverage";

/** `checklist::ChecklistState`, steps as words. */
export interface HostChecklistState {
  done: string[];
  dismissed: boolean;
  shown: boolean;
  /** Steps skipped ("Skip"). */
  skipped: string[];
}

/** What `checklist.send` takes (`checklist::ChecklistAction`, without `observe`). */
export type HostChecklistAction =
  | { type: "done"; step: string }
  | { type: "skip"; step: string }
  | { type: "shown" }
  | { type: "dismiss" }
  | { type: "reopen" }
  | { type: "merge"; state: HostChecklistState };

export interface ChecklistOps {
  "checklist.get": { args: Record<string, never>; result: { state: HostChecklistState } };
  "checklist.send": { args: { action: HostChecklistAction }; result: { state: HostChecklistState } };
}

export const CHECKLIST_OPERATIONS = ["checklist.get", "checklist.send"] as const satisfies readonly (keyof ChecklistOps)[];

export const CHECKLIST_COVERAGE = {
  "checklist.get": {
    webkit: { methods: ["checklist.get"] },
    tauri: [],
    unsupported: { tauri: "the Tauri app keeps the checklist in the page" },
  },
  "checklist.send": {
    webkit: { methods: ["checklist.send"] },
    tauri: [],
    unsupported: { tauri: "the Tauri app keeps the checklist in the page" },
  },
} as const satisfies Record<keyof ChecklistOps, OpCoverage>;

/** A host without the host's copy (the demo): the page keeps its own. */
export function unsupportedChecklistOps(mode: BridgeMode) {
  const no = (op: keyof ChecklistOps) => (): Promise<{ state: HostChecklistState }> => Promise.reject(new UnsupportedOperationError(mode, op));
  return { "checklist.get": no("checklist.get"), "checklist.send": no("checklist.send") };
}

/** Arguments that take it down its usual path (coverage tests). */
export const CHECKLIST_ARGS = {
  "checklist.get": {},
  "checklist.send": { action: { type: "done" as const, step: "teleport" } },
};
