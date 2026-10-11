// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The getting-started checklist: the first things worth doing in Cua
 * Spaces, ticked off as they happen and kept between launches (this
 * page's storage, like the first run's progress).
 *
 * Usage events are the core's (`telemetry.checklist`): `cua_onboarding_step`
 * with step `checklist_shown`, `checklist_<step>` (once per step),
 * `checklist_completed`, `checklist_dismissed` or `checklist_reopened`.
 */

import { useContext, useEffect, useSyncExternalStore } from "react";

import { BridgeContext } from "./BridgeProvider";
import type { CoreClient } from "./core";
import type { TelemetrySignal } from "./ops/telemetry";
import type { BridgeStore } from "./store";

/** The steps in order (`telemetry::CHECKLIST_STEPS`). */
export const CHECKLIST_STEPS = ["add_machine", "create_space", "teleport", "share", "run_agent"] as const;
export type ChecklistStep = (typeof CHECKLIST_STEPS)[number];

/** What happens to the checklist itself. */
export type ChecklistEvent = ChecklistStep | "shown" | "completed" | "dismissed" | "reopened";

export interface ChecklistState {
  /** Steps done, each once. */
  done: Partial<Record<ChecklistStep, true>>;
  /** Closed with ×; the menu's "Getting started" opens it again. */
  dismissed: boolean;
  /** It was on screen once (`checklist_shown` was recorded). */
  shown: boolean;
}

const STORAGE_KEY = "cua-spaces:checklist";
const EMPTY: ChecklistState = { done: {}, dismissed: false, shown: false };
let saved: ChecklistState | undefined;
const listeners = new Set<() => void>();

function storage(): Storage | undefined {
  try {
    return globalThis.localStorage;
  } catch {
    return undefined;
  }
}

function parse(raw: string | null | undefined): ChecklistState {
  const v = raw ? (JSON.parse(raw) as Partial<ChecklistState> | null) : null;
  if (!v || typeof v !== "object") return EMPTY;
  const done: ChecklistState["done"] = {};
  for (const s of CHECKLIST_STEPS) if (v.done?.[s] === true) done[s] = true;
  return { done, dismissed: v.dismissed === true, shown: v.shown === true };
}

export function readChecklist(): ChecklistState {
  if (saved) return saved;
  try {
    saved = parse(storage()?.getItem(STORAGE_KEY));
  } catch {
    saved = EMPTY;
  }
  return saved;
}

function write(next: ChecklistState): void {
  // Another window may have ticked steps since this one read them: keep those.
  try {
    const stored = parse(storage()?.getItem(STORAGE_KEY));
    next = { ...next, done: { ...stored.done, ...next.done } };
  } catch {
    // Unreadable: this window's copy wins.
  }
  saved = next;
  try {
    storage()?.setItem(STORAGE_KEY, JSON.stringify(next));
  } catch {
    // Private mode or a full disk: it still works until the app quits.
  }
  for (const l of [...listeners]) l();
}

function onStorage(e: StorageEvent): void {
  if (e.key !== STORAGE_KEY && e.key !== null) return;
  saved = undefined;
  for (const l of [...listeners]) l();
}

export function subscribeChecklist(listener: () => void): () => void {
  if (listeners.size === 0) globalThis.addEventListener?.("storage", onStorage);
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0) globalThis.removeEventListener?.("storage", onStorage);
  };
}

export const checklistComplete = (s: ChecklistState) => CHECKLIST_STEPS.every((step) => s.done[step]);

/** The core's usage events for one checklist event. */
export function telemetryChecklist(core: CoreClient, event: ChecklistEvent): TelemetrySignal[] {
  return core.tryCall<TelemetrySignal[]>("telemetry.checklist", { event }) ?? [];
}

function track(store: BridgeStore | null, events: ChecklistEvent[]): void {
  if (!store) return;
  for (const e of events) void store.telemetry.track(telemetryChecklist(store.core, e));
}

/** Events waiting for the bridge store. */
const pending: ChecklistEvent[] = [];

/* Each change is saved at once and answers with the events it means. */

function markDone(step: ChecklistStep): ChecklistEvent[] {
  const s = readChecklist();
  if (s.done[step]) return [];
  const next = { ...s, done: { ...s.done, [step]: true as const } };
  write(next);
  return checklistComplete(next) ? [step, "completed"] : [step];
}

function markShown(): ChecklistEvent[] {
  const s = readChecklist();
  if (s.shown) return [];
  write({ ...s, shown: true });
  return ["shown"];
}

function markDismissed(dismissed: boolean): ChecklistEvent[] {
  const s = readChecklist();
  if (s.dismissed === dismissed) return [];
  write({ ...s, dismissed });
  return [dismissed ? "dismissed" : "reopened"];
}

/** Ticks off `step` (once): records it, and `completed` with the last one. */
export function checklistDone(store: BridgeStore | null, step: ChecklistStep): void {
  track(store, markDone(step));
}

export interface ChecklistHook {
  state: ChecklistState;
  done(step: ChecklistStep): void;
  shown(): void;
  dismiss(): void;
  reopen(): void;
}

/** The checklist, and its actions with their usage events. */
export function useChecklist(): ChecklistHook {
  const ctx = useContext(BridgeContext);
  if (!ctx) throw new Error("useChecklist needs a <BridgeProvider> above it");
  const state = useSyncExternalStore(subscribeChecklist, readChecklist, readChecklist);
  const { store } = ctx;
  // Events from before the store exists wait for it (the first `ready`
  // promise can be replaced before it resolves).
  useEffect(() => {
    if (store && pending.length) track(store, pending.splice(0));
  }, [store]);
  const later = (events: ChecklistEvent[]) => {
    if (!events.length) return;
    if (store) track(store, events);
    else pending.push(...events);
  };
  return {
    state,
    done: (step) => later(markDone(step)),
    shown: () => later(markShown()),
    dismiss: () => later(markDismissed(true)),
    reopen: () => later(markDismissed(false)),
  };
}

/** Tests: forget the cached copy. */
export function resetChecklistCache(): void {
  saved = undefined;
  pending.length = 0;
}
