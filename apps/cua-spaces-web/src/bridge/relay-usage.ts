// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The account's relay allowance (`relay.usage`), read while a screen shows
 * it: at once, every minute, and when the app comes to the front (at
 * most once a minute). A host without the operation, signed out, or a relay
 * that meters nothing reads as null, and the page shows nothing. The words
 * are the app core's (`relayUsage.view`).
 */

import { useContext, useMemo, useSyncExternalStore } from "react";
import { isUnsupported, type DataAdapter } from "./adapter";
import { BridgeContext } from "./BridgeProvider";
import { planLimitNotice, relayUsageView } from "./derive";
import { useSettings } from "./hooks";
import type { RelayAllowanceNotice, RelayUsage, RelayUsageView } from "./ops/relay-usage";

export type { PlanCard, PlanLink, RelayAllowanceNotice, RelayUsage, RelayUsageView } from "./ops/relay-usage";

export const RELAY_USAGE_POLL_MS = 60_000;
/** A focus reads again only when the last read is older than this. */
export const RELAY_USAGE_FOCUS_MS = 60_000;

class RelayUsageStore {
  usage: RelayUsage | null = null;
  private unsupported = false;
  private readAt = 0;
  private reading = false;
  /** Bumped on every account change: an older read's answer is dropped. */
  private generation = 0;
  /** An account change came while a read was out: read again after it. */
  private again = false;
  private listeners = new Set<() => void>();
  private stop: (() => void) | null = null;

  constructor(private readonly adapter: DataAdapter) {}

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    if (this.listeners.size === 1) this.start();
    return () => {
      this.listeners.delete(listener);
      if (this.listeners.size === 0) {
        this.stop?.();
        this.stop = null;
      }
    };
  };

  snapshot = (): RelayUsage | null => this.usage;

  async read(): Promise<void> {
    if (this.unsupported) return;
    if (this.reading) return;
    this.reading = true;
    this.readAt = Date.now();
    const generation = this.generation;
    try {
      const usage = await this.adapter.call("relay.usage", {});
      if (generation === this.generation) this.set(usage);
    } catch (e) {
      // Unsupported: never again. Anything else keeps the last answer.
      if (isUnsupported(e)) {
        this.unsupported = true;
        this.set(null);
      }
    } finally {
      this.reading = false;
      if (this.again) {
        this.again = false;
        void this.read();
      }
    }
  }

  /** The account changed (signed in, out, or another one): forget the
   * last answer, then read for the current account. */
  private accountChanged() {
    this.generation += 1;
    this.set(null);
    if (this.reading) this.again = true;
    else void this.read();
  }

  private set(usage: RelayUsage | null) {
    this.usage = usage?.metered ? usage : null;
    for (const l of [...this.listeners]) l();
  }

  private start() {
    void this.read();
    const timer = setInterval(() => void this.read(), RELAY_USAGE_POLL_MS);
    const onFocus = () => {
      if (Date.now() - this.readAt >= RELAY_USAGE_FOCUS_MS) void this.read();
    };
    const win = typeof window !== "undefined" ? window : null;
    win?.addEventListener("focus", onFocus);
    const unsubscribe = this.adapter.subscribe((e) => {
      if (e.type === "session.signedOut" || e.type === "session.signedIn" || e.type === "session.changed") {
        this.accountChanged();
      }
    });
    this.stop = () => {
      clearInterval(timer);
      win?.removeEventListener("focus", onFocus);
      unsubscribe();
    };
  }
}

const stores = new WeakMap<DataAdapter, RelayUsageStore>();

function storeFor(adapter: DataAdapter): RelayUsageStore {
  let s = stores.get(adapter);
  if (!s) stores.set(adapter, (s = new RelayUsageStore(adapter)));
  return s;
}

const none = () => () => {};
const nothing = () => null;

/** The relay allowance this month, or null (nothing to show). */
export function useRelayUsage(): RelayUsage | null {
  const adapter = useContext(BridgeContext)?.adapter ?? null;
  const store = adapter ? storeFor(adapter) : null;
  return useSyncExternalStore(store?.subscribe ?? none, store?.snapshot ?? nothing, nothing);
}

/** Settings' row and the notice, as the app core draws them; null when
 * there is nothing to show (or no core to say it). */
export function useRelayUsageView(): RelayUsageView | null {
  const usage = useRelayUsage();
  const core = useContext(BridgeContext)?.core;
  const accountApp = useSettings().data?.accountUrl ?? null;
  return useMemo(() => (usage && core ? relayUsageView(core, usage, accountApp) : null), [usage, core, accountApp]);
}

/** The plan-limit refusal in an error's text, as the core's sentence with
 * its Upgrade link; null for any other error (or without the core). */
export function usePlanLimitNotice(): (error: string) => RelayAllowanceNotice | null {
  const core = useContext(BridgeContext)?.core;
  const accountApp = useSettings().data?.accountUrl ?? null;
  return useMemo(() => (error: string) => (core ? planLimitNotice(core, error, accountApp) : null), [core, accountApp]);
}
