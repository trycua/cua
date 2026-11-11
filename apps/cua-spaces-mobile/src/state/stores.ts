// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createContext, useContext, useSyncExternalStore } from "react";

import type { Cache } from "../store/cache";
import type { OutboxDriver } from "../store/outbox";

export interface Stores {
  outbox: OutboxDriver;
  cache: Cache;
}

export const StoresContext = createContext<Stores | null>(null);

export function useStores(): Stores | null {
  return useContext(StoresContext);
}

const noSubscribe = () => () => {};
const zero = () => 0;

/** The outbox, re-rendering when it changes. Null outside a provider (a screen then posts directly). */
export function useOutbox(): OutboxDriver | null {
  const s = useContext(StoresContext);
  useSyncExternalStore(s ? s.outbox.subscribe : noSubscribe, s ? s.outbox.getVersion : zero, s ? s.outbox.getVersion : zero);
  return s?.outbox ?? null;
}
