// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The getting-started checklist: the first things worth doing in Cua
 * Spaces, ticked off as they happen and kept between launches (this
 * page's storage, like the first run's progress).
 *
 * What each event does to it, its words and its usage events are the
 * core's (`checklist.reduce`, `checklist.view`), the same calls the SwiftUI
 * app makes. Usage events: `cua_onboarding_step` with step
 * `checklist_shown`, `checklist_<step>` (once per step),
 * `checklist_completed`, `checklist_dismissed` or `checklist_reopened`.
 * Without the core the checklist is not shown and nothing changes.
 */

import { useContext, useEffect, useSyncExternalStore } from "react";

import { BridgeContext } from "./BridgeProvider";
import type { CoreClient } from "./core";
import type { TelemetrySignal } from "./ops/telemetry";
import type { BridgeStore } from "./store";

/** The steps in order (`checklist::STEPS`). */
export const CHECKLIST_STEPS = ["add_machine", "create_space", "teleport", "share", "run_agent"] as const;
export type ChecklistStep = (typeof CHECKLIST_STEPS)[number];

/** As stored (the shape earlier versions wrote). */
export interface ChecklistState {
  /** Steps done, each once. */
  done: Partial<Record<ChecklistStep, true>>;
  /** Closed with ×; the menu's "Getting started" opens it again. */
  dismissed: boolean;
  /** It was on screen once (`checklist_shown` was recorded). */
  shown: boolean;
}

/** `checklist::ChecklistAction`. */
export type ChecklistAction =
  | { type: "done"; step: ChecklistStep }
  | { type: "shown" }
  | { type: "dismiss" }
  | { type: "reopen" }
  | { type: "observe"; otherMachine: boolean; hasSpace: boolean; agentRan: boolean };

/** `checklist::ChecklistRow`. */
export interface ChecklistRow {
  step: ChecklistStep;
  title: string;
  detail: string;
  /** The button that does it; hidden once done. */
  action: string;
  done: boolean;
}

/** `checklist::ChecklistView`. */
export interface ChecklistView {
  visible: boolean;
  complete: boolean;
  title: string;
  detail: string;
  /** "2 of 5". */
  progress: string;
  closeLabel: string;
  rows: ChecklistRow[];
}

/** `checklist::ChecklistState`. */
interface CoreChecklistState {
  done: ChecklistStep[];
  dismissed: boolean;
  shown: boolean;
}

const toCore = (s: ChecklistState): CoreChecklistState => ({
  done: CHECKLIST_STEPS.filter((step) => s.done[step]),
  dismissed: s.dismissed,
  shown: s.shown,
});

function fromCore(c: CoreChecklistState): ChecklistState {
  const done: ChecklistState["done"] = {};
  for (const step of c.done) done[step] = true;
  return { done, dismissed: c.dismissed, shown: c.shown };
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

/** The checklist as drawn, or null without the core. */
export function checklistView(core: CoreClient, state: ChecklistState): ChecklistView | null {
  return core.tryCall<ChecklistView>("checklist.view", { state: toCore(state) }) ?? null;
}

/** Runs `action` through the core: saves what changed and answers the
 * usage events it means. */
function apply(core: CoreClient, action: ChecklistAction): TelemetrySignal[] {
  const t = core.tryCall<{ state: CoreChecklistState; signals: TelemetrySignal[] }>("checklist.reduce", {
    state: toCore(readChecklist()),
    action,
  });
  if (!t || t.signals.length === 0) return [];
  write(fromCore(t.state));
  return t.signals;
}

/** Usage events waiting for the bridge store. */
const pending: TelemetrySignal[] = [];

/** Ticks off `step` (once): records it, and `completed` with the last one. */
export function checklistDone(store: BridgeStore | null, step: ChecklistStep): void {
  if (!store) return;
  const signals = apply(store.core, { type: "done", step });
  if (signals.length) void store.telemetry.track(signals);
}

export interface ChecklistHook {
  state: ChecklistState;
  /** Null without the core. */
  view: ChecklistView | null;
  send(action: ChecklistAction): void;
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
  const { core, store } = ctx;
  // Events from before the store exists wait for it (the first `ready`
  // promise can be replaced before it resolves).
  useEffect(() => {
    if (store && pending.length) void store.telemetry.track(pending.splice(0));
  }, [store]);
  const send = (action: ChecklistAction) => {
    const signals = apply(core, action);
    if (!signals.length) return;
    if (store) void store.telemetry.track(signals);
    else pending.push(...signals);
  };
  return {
    state,
    view: checklistView(core, state),
    send,
    done: (step) => send({ type: "done", step }),
    shown: () => send({ type: "shown" }),
    dismiss: () => send({ type: "dismiss" }),
    reopen: () => send({ type: "reopen" }),
  };
}

/** Tests: forget the cached copy. */
export function resetChecklistCache(): void {
  saved = undefined;
  pending.length = 0;
}
