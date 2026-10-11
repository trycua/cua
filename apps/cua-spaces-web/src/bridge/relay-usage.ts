// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The account's relay allowance (`relay.usage`), read while a screen shows
 * it: at once, every 5 minutes, and when the app comes to the front (at
 * most once a minute). A host without the operation, signed out, or a relay
 * that meters nothing reads as null, and the page shows nothing.
 */

import { useContext, useSyncExternalStore } from "react";
import { isUnsupported, type DataAdapter } from "./adapter";
import { BridgeContext } from "./BridgeProvider";
import type { RelayUsage } from "./ops/relay-usage";

export type { RelayUsage } from "./ops/relay-usage";

export const RELAY_USAGE_POLL_MS = 5 * 60_000;
/** A focus reads again only when the last read is older than this. */
export const RELAY_USAGE_FOCUS_MS = 60_000;

class RelayUsageStore {
  usage: RelayUsage | null = null;
  private unsupported = false;
  private readAt = 0;
  private reading = false;
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
    if (this.unsupported || this.reading) return;
    this.reading = true;
    this.readAt = Date.now();
    try {
      this.set(await this.adapter.call("relay.usage", {}));
    } catch (e) {
      // Unsupported: never again. Anything else keeps the last answer.
      if (isUnsupported(e)) {
        this.unsupported = true;
        this.set(null);
      }
    } finally {
      this.reading = false;
    }
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
      if (e.type === "session.signedOut") this.set(null);
      else if (e.type === "session.signedIn") void this.read();
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

/** `20` or `6.5`. */
export function formatHours(h: number): string {
  const tenths = Math.round(h * 10) / 10;
  return Number.isInteger(tenths) ? String(tenths) : tenths.toFixed(1);
}

/** "Live sessions 1 of 2 · Relay hours 6.5 of 20 this month". */
export function relayUsageLine(u: RelayUsage): string {
  const of = (used: string, limit: string | null) => (limit === null ? used : `${used} of ${limit}`);
  const sessions = of(String(u.liveSessions), u.liveSessionsLimit === null ? null : String(u.liveSessionsLimit));
  const hours = of(formatHours(u.hoursUsed), u.hoursLimit === null ? null : formatHours(u.hoursLimit));
  return `Live sessions ${sessions} · Relay hours ${hours} this month`;
}

