// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The getting-started checklist: the first things worth doing in Cua
 * Spaces, ticked off as they happen and kept between launches.
 *
 * In the Mac app and the Electron app the host keeps the one copy
 * (`ops/checklist.ts`: `checklist.get`, `checklist.send`), runs the core's
 * reducer, records the usage events and ticks steps off from what it
 * shows (`checklist::observe_from`), so the classic Mac UI, the New UI and
 * Electron count the same. Progress this page kept itself before (its
 * storage) is merged into the host's copy once, then forgotten here.
 *
 * Elsewhere (the demo, Tauri, an older host) the page keeps its own copy
 * in its storage. What each event does to it, its words and its usage
 * events are the core's (`checklist.reduce`, `checklist.view`). Usage
 * events: `cua_onboarding_step` with step `checklist_shown`,
 * `checklist_<step>` (once per step), `checklist_completed`,
 * `checklist_dismissed` or `checklist_reopened`. Without the core the
 * checklist is not shown and nothing changes.
 */

import { useContext, useEffect, useSyncExternalStore } from "react";

import { isNativeHost, type DataAdapter } from "./adapter";
import { BridgeContext } from "./BridgeProvider";
import type { CoreClient } from "./core";
import type { HostChecklistAction, HostChecklistState } from "./ops/checklist";
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
  /** Steps skipped ("Skip"): they count as done. */
  skipped: Partial<Record<ChecklistStep, true>>;
}

/** `checklist::ChecklistAction`. */
export type ChecklistAction =
  | { type: "done"; step: ChecklistStep }
  | { type: "skip"; step: ChecklistStep }
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
  /** Skipped (never with `done`). */
  skipped: boolean;
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
  /** "Skip", on each step not done or skipped. */
  skipLabel: string;
  /** "Hide", or "Done" once complete. */
  hideLabel: string;
  rows: ChecklistRow[];
}

/** `checklist::ChecklistState`. */
interface CoreChecklistState {
  done: ChecklistStep[];
  dismissed: boolean;
  shown: boolean;
  skipped: ChecklistStep[];
}

const toCore = (s: ChecklistState): CoreChecklistState => ({
  done: CHECKLIST_STEPS.filter((step) => s.done[step]),
  dismissed: s.dismissed,
  shown: s.shown,
  skipped: CHECKLIST_STEPS.filter((step) => s.skipped[step]),
});

function steps(list: readonly string[] | undefined): Partial<Record<ChecklistStep, true>> {
  const out: Partial<Record<ChecklistStep, true>> = {};
  for (const step of list ?? []) if ((CHECKLIST_STEPS as readonly string[]).includes(step)) out[step as ChecklistStep] = true;
  return out;
}

function fromCore(c: CoreChecklistState | HostChecklistState): ChecklistState {
  // An older host answers without `skipped`.
  return { done: steps(c.done), dismissed: c.dismissed === true, shown: c.shown === true, skipped: steps(c.skipped) };
}

const STORAGE_KEY = "cua-spaces:checklist";
const EMPTY: ChecklistState = { done: {}, dismissed: false, shown: false, skipped: {} };
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
  const skipped: ChecklistState["skipped"] = {};
  for (const s of CHECKLIST_STEPS) {
    if (v.done?.[s] === true) done[s] = true;
    if (v.skipped?.[s] === true) skipped[s] = true;
  }
  return { done, dismissed: v.dismissed === true, shown: v.shown === true, skipped };
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
    next = { ...next, done: { ...stored.done, ...next.done }, skipped: { ...stored.skipped, ...next.skipped } };
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

/* ---- The host's copy ------------------------------------------------------ */

/** The host's copy of the checklist (Mac app, Electron). `state` is
 * undefined until the host answered; `failed`: it never did (an older
 * host), and the page keeps its own copy. */
class HostChecklist {
  state: ChecklistState | undefined;
  failed = false;
  private started = false;
  private unsubscribe?: () => void;

  constructor(readonly adapter: DataAdapter) {}

  start(): void {
    if (this.started) return;
    this.started = true;
    this.unsubscribe = this.adapter.subscribe((e) => {
      if (e.type === "checklist.changed") void this.load(false);
    });
    void this.load(true);
  }

  stop(): void {
    this.unsubscribe?.();
  }

  private set(next: HostChecklistState): void {
    this.state = fromCore(next);
    for (const l of [...listeners]) l();
  }

  private async load(first: boolean): Promise<void> {
    try {
      this.set((await this.adapter.call("checklist.get", {})).state);
    } catch {
      if (this.state === undefined) {
        this.failed = true;
        for (const l of [...listeners]) l();
      }
      return;
    }
    if (first) await this.migrate();
  }

  /** Progress this page kept itself (its storage), merged into the
   * host's copy once and then forgotten here. */
  private async migrate(): Promise<void> {
    let kept: ChecklistState;
    try {
      kept = parse(storage()?.getItem(STORAGE_KEY));
    } catch {
      return;
    }
    if (!kept.dismissed && !kept.shown && Object.keys(kept.done).length === 0 && Object.keys(kept.skipped).length === 0) return;
    try {
      await this.send({ type: "merge", state: toCore(kept) });
      storage()?.removeItem(STORAGE_KEY);
      saved = undefined;
    } catch {
      // Kept for the next launch.
    }
  }

  async send(action: HostChecklistAction): Promise<void> {
    this.set((await this.adapter.call("checklist.send", { action })).state);
  }
}

let host: HostChecklist | undefined;

/** The host's copy when the host keeps one; undefined: the page keeps its own. */
function hostChecklist(adapter: DataAdapter | null | undefined): HostChecklist | undefined {
  if (!adapter || !isNativeHost(adapter.mode)) return undefined;
  if (host?.adapter !== adapter) {
    host?.stop();
    host = new HostChecklist(adapter);
  }
  return host.failed ? undefined : host;
}

/** What the host takes: everything but `observe` (it observes itself). */
function forHost(action: ChecklistAction): HostChecklistAction | null {
  return action.type === "observe" ? null : action;
}

const LOADING: ChecklistState = { done: {}, dismissed: true, shown: true, skipped: {} };

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
  if (!t) return [];
  // A skip changes the state without an event of its own.
  const next = fromCore(t.state);
  if (t.signals.length || JSON.stringify(toCore(next)) !== JSON.stringify(toCore(readChecklist()))) write(next);
  return t.signals;
}

/** Usage events waiting for the bridge store. */
const pending: TelemetrySignal[] = [];

/** Ticks off `step` (once): records it, and `completed` with the last one. */
export function checklistDone(store: BridgeStore | null, step: ChecklistStep): void {
  if (!store) return;
  const h = hostChecklist(store.adapter);
  if (h) {
    h.start();
    void h.send({ type: "done", step }).catch(() => {});
    return;
  }
  const signals = apply(store.core, { type: "done", step });
  if (signals.length) void store.telemetry.track(signals);
}

export interface ChecklistHook {
  state: ChecklistState;
  /** Null without the core. */
  view: ChecklistView | null;
  send(action: ChecklistAction): void;
  done(step: ChecklistStep): void;
  /** "Skip" on a step: it counts as done. */
  skip(step: ChecklistStep): void;
  shown(): void;
  dismiss(): void;
  reopen(): void;
}

/** The checklist, and its actions with their usage events. */
export function useChecklist(): ChecklistHook {
  const ctx = useContext(BridgeContext);
  if (!ctx) throw new Error("useChecklist needs a <BridgeProvider> above it");
  const { core, store } = ctx;
  const h = hostChecklist(ctx.adapter ?? store?.adapter);
  useEffect(() => h?.start(), [h]);
  const read = () => (h ? (h.failed ? readChecklist() : (h.state ?? LOADING)) : readChecklist());
  const state = useSyncExternalStore(subscribeChecklist, read, read);
  const loading = Boolean(h && !h.failed && h.state === undefined);
  // Events from before the store exists wait for it (the first `ready`
  // promise can be replaced before it resolves).
  useEffect(() => {
    if (store && pending.length) void store.telemetry.track(pending.splice(0));
  }, [store]);
  const send = (action: ChecklistAction) => {
    if (h && !h.failed) {
      const a = forHost(action);
      if (a) void h.send(a).catch(() => {});
      return;
    }
    const signals = apply(core, action);
    if (!signals.length) return;
    if (store) void store.telemetry.track(signals);
    else pending.push(...signals);
  };
  return {
    state,
    // Hidden until the host answered (no flash of an empty checklist).
    view: loading ? null : checklistView(core, state),
    send,
    done: (step) => send({ type: "done", step }),
    skip: (step) => send({ type: "skip", step }),
    shown: () => send({ type: "shown" }),
    dismiss: () => send({ type: "dismiss" }),
    reopen: () => send({ type: "reopen" }),
  };
}

/** Tests: forget the cached copy. */
export function resetChecklistCache(): void {
  saved = undefined;
  pending.length = 0;
  host?.stop();
  host = undefined;
}
