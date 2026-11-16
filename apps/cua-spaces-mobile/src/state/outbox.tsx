// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect, useMemo, type ReactNode } from "react";

import { Cache } from "../store/cache";
import { OutboxDriver } from "../store/outbox";
import type { SealedStore } from "../store/sealed";
import { useBackend, useForeground } from "./backend";
import { StoresContext, type Stores } from "./stores";

/**
 * The outbox and the cache for the signed-in account, over one sealed store. Mounted inside the backend provider; the
 * driver reaches whichever backend is current. `me`, `now` and `newId` must be stable (they key the driver); a test gives a clock and ids it controls.
 */
export function StoresProvider({
  store,
  me,
  children,
  now,
  newId,
}: {
  store: SealedStore;
  me: () => string | null;
  children: ReactNode;
  now?: () => number;
  newId?: () => string;
}) {
  const backend = useBackend();
  const foreground = useForeground();
  const stores = useMemo<Stores>(
    () => ({ outbox: new OutboxDriver({ store, backend: () => backend, me, now, newId }), cache: new Cache(store, now) }),
    [store, backend, me, now, newId],
  );
  // Coming to the foreground retries what is due; going to the background saves what was about to be.
  useEffect(() => {
    if (foreground) stores.outbox.tick();
    else void stores.cache.flush();
  }, [foreground, stores]);
  return <StoresContext.Provider value={stores}>{children}</StoresContext.Provider>;
}
