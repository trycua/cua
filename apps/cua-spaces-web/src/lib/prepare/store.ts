// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useMemo, useSyncExternalStore } from "react";

import type { CoreClient } from "@/bridge/core";
import type { PrepareMessage, PrepareState, PrepareView } from "@/bridge/contracts/prepare";
import { useSpaces } from "@/bridge";
import { prepareHidden } from "@/bridge/prepare-core";
import { spaceState } from "@/lib/spaces";

import type { PrepareDriver } from "./driver";

/** The warming home's fixed name for a coding app: a create the host still runs is found by it. */
export const homeNameOf = (harness: string) => `home-${harness}`;

/** Where the driver is kept: one for the page, made once the app core is there. */
let driver: PrepareDriver | null = null;

/** The page's driver (none until the app core is there). */
export const getDriver = (): PrepareDriver | null => driver;
export function setDriver(d: PrepareDriver | null): void {
  driver = d;
  d?.subscribe(() => changed());
  notify();
  changed();
}

/** Non-React readers (the bridge store) hear when what the lists hide may have changed. */
const changeListeners = new Set<() => void>();
const changed = () => {
  for (const l of [...changeListeners]) l();
};
export function onHiddenSpacesChange(cb: () => void): () => void {
  changeListeners.add(cb);
  return () => changeListeners.delete(cb);
}

/** The Spaces no list shows right now (the core's `prepare.hidden` over the saved record). */
export function hiddenSpaceIdsNow(core: CoreClient): string[] {
  return prepareHidden(core, driver?.state ?? null);
}
const listeners = new Set<() => void>();
const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => listeners.delete(l);
};
export const notify = () => {
  for (const l of [...listeners]) l();
};

/** A saved record was resumed in this page load (the layout mounts again and again: once is enough). */
let resumedOnce = false;
export const resumedThisLoad = (): boolean => resumedOnce;
export function markResumed(): void {
  resumedOnce = true;
}

/**
 * Tests that do not look at the warm home keep it from starting, so their mounts make no Space of their own (the
 * test harness holds it for every test; the tests of the warm home let it go).
 */
let held = false;
export const holdWarmStart = (on: boolean): void => {
  held = on;
};
export const warmStartHeld = (): boolean => held;

/** Tests start clean. */
export function resetPrepare(): void {
  driver = null;
  resumedOnce = false;
  notify();
}

/** The driver's state, as the page reads it. */
export function usePrepare(): { state: PrepareState | null; view: PrepareView | null; driver: PrepareDriver | null } {
  const d = useSyncExternalStore(
    subscribe,
    () => driver,
    () => null,
  );
  useSyncExternalStore(
    d ? d.subscribe : (l) => (listeners.add(l), () => listeners.delete(l)),
    () => d?.state ?? null,
    () => null,
  );
  // Properties, not method calls: they change with every step, and the compiler caches a call by its receiver.
  return { state: d?.state ?? null, view: d?.pageView ?? null, driver: d };
}

/** The id the home's page has now: its Space, else the row it has while it is made. */
export function prepareSpaceId(state: PrepareState | null): string | null {
  return state?.wanted ? (state.homeSpaceId ?? state.pendingId) : null;
}

/** Whether `spaceId` is the home being prepared (its row, or its Space). */
export function usePreparingSpace(spaceId: string | undefined): PrepareState | null {
  const { state } = usePrepare();
  return state?.wanted && spaceId && (state.pendingId === spaceId || state.homeSpaceId === spaceId) ? state : null;
}

/** The messages kept for the home that are not in its thread yet. */
export function usePreparedQueue(spaceId: string): PrepareMessage[] {
  const { state } = usePrepare();
  return useMemo(
    () => (state?.wanted && (state.pendingId === spaceId || state.homeSpaceId === spaceId) ? state.messages.filter((m) => m.state !== "delivered") : []),
    [state, spaceId],
  );
}

/**
 * The Spaces no list shows: the home that warms for Home's box until a person wrote into it (the core's
 * `prepare.hidden`, one rule for every app). Its own page and Home's preparing line still show it.
 */
export function useHiddenSpaceIds(core: CoreClient): ReadonlySet<string> {
  const { state } = usePrepare();
  const { data: spaces } = useSpaces();
  return useMemo(() => {
    const ids = new Set(prepareHidden(core, state));
    // The host's own mark (label `cua.hidden`, cleared when a person writes into the home's thread) hides it even when
    // this page holds no record of preparing it: cleared site data, another window, a home an older build made.
    for (const s of spaces ?? []) if (s.hidden) ids.add(s.id);
    // A create that outlived the app (it was quit meanwhile) is followed before the driver knows its id: still the
    // warming home, so still off every list.
    if (state?.wanted && state.askedMs == null && state.harness) {
      const name = homeNameOf(state.harness);
      for (const s of spaces ?? []) if (s.name === name && spaceState(s) === "creating") ids.add(s.id);
    }
    return ids;
  }, [core, state, spaces]);
}

/** A person wrote in this Space's own thread: when it is the warming home, it is listed from now on. */
export function markAsked(spaceId: string): void {
  const d = driver;
  if (d?.state?.wanted && (d.state.homeSpaceId === spaceId || d.state.pendingId === spaceId)) d.asked();
}
