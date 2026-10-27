// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Tips as remembered, and what changes them (CUA-2204). They replace the
 * getting-started checklist and the agent banner.
 *
 * The state is the core's `AppSettings.tips` (`{ off, done }`), kept by the
 * host with the rest of the app's settings (`settings.get` answers it in
 * `values.tips`; `settings.set` with key `tips` saves it), so the SwiftUI
 * app's classic UI, its New UI and the Electron app agree. What an event
 * does to it is the core's (`tips.reduce`); which tip shows is the core's
 * too (`tips.view`, drawn by `components/tips.tsx`).
 *
 * A host that keeps no core settings (an older one) refuses the save: the
 * page then keeps the state for this session, so a tip closed stays closed
 * until the app is opened again.
 */

import { useCallback, useContext, useSyncExternalStore } from "react";

import { BridgeContext } from "./BridgeProvider";
import type { TipsEvent, TipsState } from "./contracts/spaces-page";
import type { BridgeStore, SettingsData } from "./store";

/** Nothing done, tips on (the core's default). */
export const NO_TIPS: TipsState = { off: false, done: [] };

/** The state per store when the host refused to keep it. */
const kept = new WeakMap<BridgeStore, TipsState>();
const listeners = new Set<() => void>();
const changed = () => {
  for (const l of listeners) l();
};

/** The tips now: the host's copy, or this session's when the host keeps none. */
export function tipsState(store: BridgeStore): TipsState {
  return kept.get(store) ?? store.get<SettingsData>("settings").data?.values.tips ?? NO_TIPS;
}

/** Saves `next` as the tips (the host's settings, else this session). */
export async function saveTips(store: BridgeStore, next: TipsState): Promise<void> {
  if (kept.has(store)) {
    kept.set(store, next);
    changed();
    return;
  }
  try {
    await store.updateSetting("tips", next);
    // Saved, but the host answers without them: it keeps no core settings.
    if (store.get<SettingsData>("settings").data?.values.tips === undefined) {
      kept.set(store, next);
      changed();
    }
  } catch {
    kept.set(store, next);
    changed();
  }
}

/** Applies `event` (the core's `tips.reduce`) and saves the result. Without
 * the core nothing changes. */
export async function sendTipsEvent(store: BridgeStore, event: TipsEvent): Promise<void> {
  if (store.core.status !== "ready") return;
  const before = tipsState(store);
  // A tip never gets in the way of what finished it (a share, a power change): a core
  // without tips, or one that refuses, changes nothing.
  let after: TipsState | undefined;
  try {
    after = store.core.tryCall<TipsState>("tips.reduce", { state: before, event });
  } catch {
    return;
  }
  if (!after) return;
  if (after.off === before.off && after.done.length === before.done.length && after.done.every((d, i) => d === before.done[i])) return;
  await saveTips(store, after);
}

const noop = () => () => {};

export interface TipsHook {
  state: TipsState;
  /** A Next tip, ×, Don't show tips, or a tip's work done elsewhere. */
  send(event: TipsEvent): Promise<void>;
  /** Saves the tips as given (Settings' Show tips and Reset tips, through `settings.choose`). */
  save(next: TipsState): Promise<void>;
}

/** The tips now, and what changes them. */
export function useTips(): TipsHook {
  const ctx = useContext(BridgeContext);
  if (!ctx) throw new Error("useTips needs a <BridgeProvider> above it");
  const store = ctx.store;
  const subscribe = useCallback(
    (l: () => void) => {
      if (!store) return noop();
      listeners.add(l);
      const off = store.subscribe(l);
      return () => {
        listeners.delete(l);
        off();
      };
    },
    [store],
  );
  const state = useSyncExternalStore(subscribe, () => (store ? tipsState(store) : NO_TIPS), () => NO_TIPS);
  return {
    state,
    send: (event) => ctx.ready.then((s) => sendTipsEvent(s, event)),
    save: (next) => ctx.ready.then((s) => saveTips(s, next)),
  };
}
