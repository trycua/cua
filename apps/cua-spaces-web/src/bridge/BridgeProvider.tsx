// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createContext, useEffect, useState, type ReactNode } from "react";
import type { BridgeMode, DataAdapter } from "./adapter";
import { createAdapter } from "./adapters";
import { loadCore, unavailableCore, type CoreClient } from "./core";
import { detectMode } from "./detect";
import { installParityHandle } from "./parity";
import { BridgeStore, type StoreOptions } from "./store";

export interface BridgeContextValue {
  mode: BridgeMode;
  /** The app core: `loading`, then `ready` (wasm) or `unavailable`. */
  core: CoreClient;
  adapter: DataAdapter | null;
  store: BridgeStore | null;
  /** Resolves once the core settled and the store exists. */
  ready: Promise<BridgeStore>;
}

export const BridgeContext = createContext<BridgeContextValue | null>(null);

export interface BridgeProviderProps {
  children?: ReactNode;
  /** Force a host (default: detected; `?bridge=demo` forces demo). */
  mode?: BridgeMode;
  /** Use this adapter instead of creating one (tests, stories). */
  adapter?: DataAdapter;
  /** Use this core instead of loading the wasm (tests). */
  core?: CoreClient;
  storeOptions?: StoreOptions;
}

function deferred<T>() {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((r) => (resolve = r));
  return { promise, resolve };
}

/**
 * Detects the host, loads the app core and owns the bridge store. Renders
 * its children at once; hooks report `isLoading` until data arrives.
 */
export function BridgeProvider({ children, mode, adapter, core, storeOptions }: BridgeProviderProps) {
  const [value, setValue] = useState<BridgeContextValue>(() => {
    const d = deferred<BridgeStore>();
    return {
      mode: adapter?.mode ?? mode ?? detectMode(),
      core: core ?? unavailableCore("loading", "loading"),
      adapter: null,
      store: null,
      ready: d.promise,
    };
  });

  useEffect(() => {
    let live = true;
    const d = deferred<BridgeStore>();
    const data = adapter ?? createAdapter(mode ?? detectMode());
    let store: BridgeStore | null = null;
    let uninstallParity = () => {};
    setValue((v) => ({ ...v, mode: data.mode, adapter: data, store: null, ready: d.promise }));
    void (core ? Promise.resolve(core) : loadCore()).then((c) => {
      if (!live) return;
      store = new BridgeStore(data, c, storeOptions);
      uninstallParity = installParityHandle(store);
      store.launched();
      setValue({ mode: data.mode, core: c, adapter: data, store, ready: d.promise });
      d.resolve(store);
    });
    return () => {
      live = false;
      uninstallParity();
      store?.dispose();
      if (!adapter) data.dispose?.();
    };
    // storeOptions is read once per adapter.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [adapter, core, mode]);

  return <BridgeContext.Provider value={value}>{children}</BridgeContext.Provider>;
}
