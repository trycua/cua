// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The Space's network settings from the host's store, and which Space's
// Network sheet is open.

import { useSyncExternalStore } from "react";

import { useBridge } from "@/bridge";
import type { NetworkPreview } from "@/bridge/contracts/network";

const noop = () => () => {};

/** The host's store, re-rendering on every change; null on a host without it. */
export function useNetworkPreview(): NetworkPreview | null {
  const { data } = useBridge();
  const store = data?.network ?? null;
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

/** Opens a Space's Network sheet (from its Network row). */
export const openNetworkSheet = (spaceId: string) => set(spaceId);
export const closeNetworkSheet = () => set(null);

export function useNetworkSheetFor(): string | null {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => openFor,
    () => null,
  );
}
