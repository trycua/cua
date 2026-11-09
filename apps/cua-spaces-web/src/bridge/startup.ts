// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * `useStartup()`: whether the native app is still starting (the startup
 * screen) and its buttons. Reads the adapter directly, not the store, so it
 * answers before the app core has loaded. A failure reads as ready: the
 * startup screen never blocks the app on a host that can't say.
 */

import { useCallback, useContext, useEffect, useState } from "react";
import { BridgeContext } from "./BridgeProvider";
import { READY_STARTUP, startupFromHost, type StartupAction, type StartupState } from "./ops/startup";

export interface StartupHook {
  data: StartupState;
  /** No answer yet (show the app as usual meanwhile). */
  isLoading: boolean;
  error: Error | null;
  refresh(): Promise<void>;
  act(action: StartupAction): Promise<void>;
}

const asError = (e: unknown) => (e instanceof Error ? e : new Error(String(e)));

export function useStartup(): StartupHook {
  const ctx = useContext(BridgeContext);
  if (!ctx) throw new Error("bridge hooks need a <BridgeProvider> above them");
  const { adapter } = ctx;
  const [state, setState] = useState<{ data: StartupState; isLoading: boolean; error: Error | null }>({
    data: READY_STARTUP,
    isLoading: true,
    error: null,
  });

  const refresh = useCallback(async () => {
    if (!adapter) return;
    try {
      const data = startupFromHost(await adapter.call("startup.get", {}));
      setState({ data, isLoading: false, error: null });
    } catch (e) {
      setState({ data: READY_STARTUP, isLoading: false, error: asError(e) });
    }
  }, [adapter]);

  useEffect(() => {
    if (!adapter) return;
    let live = true;
    const off = adapter.subscribe((e) => {
      if (live && e.type === "startup.changed") setState({ data: startupFromHost(e.state), isLoading: false, error: null });
    });
    void adapter.call("startup.get", {}).then(
      (data) => live && setState({ data: startupFromHost(data), isLoading: false, error: null }),
      (e: unknown) => live && setState({ data: READY_STARTUP, isLoading: false, error: asError(e) }),
    );
    return () => {
      live = false;
      off();
    };
  }, [adapter]);

  const act = useCallback(
    async (action: StartupAction) => {
      if (!adapter) return;
      try {
        const data = startupFromHost(await adapter.call("startup.act", { action }));
        setState({ data, isLoading: false, error: null });
      } catch (e) {
        setState({ data: READY_STARTUP, isLoading: false, error: asError(e) });
      }
    },
    [adapter],
  );

  return { ...state, refresh, act };
}
