// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The account's default Space (`spaces.settings`, `spaces.resolveDefault`,
 * ops/spaces-settings.ts), read while a screen shows it: at once, when a
 * computer or a Space changes (one may have gone offline), when the account
 * changes, and after a change made here. A host without the operations, or
 * signed out, reads as null: the screens fall back to the tree's own order
 * (`lib/spaces-tree.ts`).
 */

import { useContext, useSyncExternalStore } from "react";

import { isUnsupported, type DataAdapter } from "./adapter";
import { BridgeContext } from "./BridgeProvider";
import type { DefaultSpace, SpacesSettings, SpacesSettingsInput } from "./ops/spaces-settings";

export type { DefaultCandidate, DefaultSpace, DefaultSpaceReason, DefaultSpaceStep, SpacesSettings, SpacesSettingsInput } from "./ops/spaces-settings";
export { resolveDefaultSpace } from "./ops/spaces-settings";

export interface SpacesDefaultSnapshot {
  settings: SpacesSettings | null;
  resolved: DefaultSpace | null;
  /** The first read answered (or failed). */
  loaded: boolean;
  /** This host has no default Space. */
  unsupported: boolean;
}

const EMPTY: SpacesDefaultSnapshot = { settings: null, resolved: null, loaded: false, unsupported: false };

class SpacesDefaultStore {
  snap: SpacesDefaultSnapshot = EMPTY;
  private listeners = new Set<() => void>();
  private stop: (() => void) | null = null;
  private generation = 0;

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

  snapshot = (): SpacesDefaultSnapshot => this.snap;

  async read(): Promise<void> {
    if (this.snap.unsupported) return;
    const generation = ++this.generation;
    try {
      const [settings, resolved] = await Promise.all([this.adapter.call("spaces.settings", {}), this.adapter.call("spaces.resolveDefault", {})]);
      if (generation === this.generation) this.set({ settings, resolved, loaded: true, unsupported: false });
    } catch (e) {
      if (generation !== this.generation) return;
      // Unsupported: never again. Anything else keeps the last answer.
      this.set(isUnsupported(e) ? { ...EMPTY, loaded: true, unsupported: true } : { ...this.snap, loaded: true });
    }
  }

  /** Replaces the setting, then reads the resolution again. */
  async save(next: SpacesSettingsInput): Promise<void> {
    const saved = await this.adapter.call("spaces.setSettings", { defaultSpace: next.defaultSpace, fallback: [...next.fallback], fallbackOn: next.fallbackOn });
    this.set({ ...this.snap, settings: saved });
    await this.read();
  }

  private set(snap: SpacesDefaultSnapshot) {
    this.snap = snap;
    for (const l of [...this.listeners]) l();
  }

  private start() {
    void this.read();
    const unsubscribe = this.adapter.subscribe((e) => {
      if (
        e.type === "machines.changed" ||
        e.type === "spaces.changed" ||
        e.type === "session.signedIn" ||
        e.type === "session.signedOut" ||
        e.type === "session.changed"
      ) {
        void this.read();
      }
    });
    this.stop = unsubscribe;
  }
}

const stores = new WeakMap<DataAdapter, SpacesDefaultStore>();

function storeFor(adapter: DataAdapter): SpacesDefaultStore {
  let s = stores.get(adapter);
  if (!s) stores.set(adapter, (s = new SpacesDefaultStore(adapter)));
  return s;
}

const none = () => () => {};
const empty = () => EMPTY;

export interface SpacesDefaultHook extends SpacesDefaultSnapshot {
  /** Replaces the whole setting (the relay keeps it for the account). */
  save(next: SpacesSettingsInput): Promise<void>;
  /** "Make this my default": only the default changes. */
  makeDefault(space: string): Promise<void>;
  refresh(): Promise<void>;
}

/** The default Space and where a new task goes now. `enabled` false reads nothing (the Labs switch is off). */
export function useSpacesDefault(enabled = true): SpacesDefaultHook {
  const adapter = useContext(BridgeContext)?.adapter ?? null;
  const store = enabled && adapter ? storeFor(adapter) : null;
  const snap = useSyncExternalStore(store?.subscribe ?? none, store?.snapshot ?? empty, empty);
  const settings = snap.settings;
  return {
    ...snap,
    save: async (next) => {
      if (!store) throw new Error("Not connected.");
      await store.save(next);
    },
    makeDefault: async (space) => {
      if (!store) throw new Error("Not connected.");
      // The fallback order stays; the new default leaves it (it is tried first anyway).
      const fallback = (settings?.fallback ?? []).filter((id) => id !== space);
      await store.save({ defaultSpace: space, fallback, fallbackOn: settings?.fallbackOn ?? true });
    },
    refresh: async () => {
      await store?.read();
    },
  };
}
