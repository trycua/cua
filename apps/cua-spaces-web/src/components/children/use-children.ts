// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Child Spaces from the host's store, whether the experiment is on, and
// which Space's Children sheet is open.

import { useSyncExternalStore } from "react";

import { useBridge } from "@/bridge";
import type { ChildrenPreview } from "@/bridge/contracts/children";
import { useExperiments } from "@/bridge/settings-hooks";

const noop = () => () => {};

/** The host's store while the Child Spaces experiment is on, re-rendering
 * on every change; null otherwise (and on a host without it). */
export function useChildren(): ChildrenPreview | null {
  const { data } = useBridge();
  const on = useExperiments().data?.experiments.childSpaces === true;
  const store = on ? (data?.children ?? null) : null;
  useSyncExternalStore(
    store ? store.subscribe : noop,
    () => store?.version() ?? 0,
    () => 0,
  );
  return store;
}

let openFor: string | null = null;
const listeners = new Set<() => void>();
const set = (id: string | null) => {
  openFor = id;
  for (const l of listeners) l();
};

/** Opens a Space's Children sheet (from its Children row). */
export const openChildrenSheet = (spaceId: string) => set(spaceId);
export const closeChildrenSheet = () => set(null);

export function useChildrenSheetFor(): string | null {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => openFor,
    () => null,
  );
}
