// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Processor, memory and disk numbers (`resources.get`), read while a screen
 * shows them: every 10 s while the window is visible, every 5 minutes while
 * it is hidden, at once when it comes back, and when the Spaces change. The
 * read says whether the window is visible, so the daemon samples at the
 * same pace. A host that doesn't answer the operation reads as null, and
 * the app shows no numbers.
 */

import { useContext, useSyncExternalStore } from "react";
import { HostError, isUnsupported, type DataAdapter } from "./adapter";
import { BridgeContext } from "./BridgeProvider";
import type { ComputerResources, ResourcesSnapshot, SpaceResources } from "./ops/resources";

export type { ComputerResources, DiskOnHost, DiskWarning, GuestUsage, ResourcesSnapshot, SpaceResources } from "./ops/resources";

export const RESOURCES_VISIBLE_MS = 10_000;
export const RESOURCES_HIDDEN_MS = 5 * 60_000;

const hidden = () => typeof document !== "undefined" && document.visibilityState === "hidden";

/** A host built before the operation: it never will answer. */
const notThere = (e: unknown) => isUnsupported(e) || (e instanceof HostError && e.code === "unimplemented");

class ResourcesStore {
  private snap: ResourcesSnapshot | null = null;
  private unsupported = false;
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

  snapshot = (): ResourcesSnapshot | null => this.snap;

  async read(): Promise<void> {
    if (this.unsupported || this.reading) return;
    this.reading = true;
    try {
      this.set(await this.adapter.call("resources.get", { visible: !hidden() }));
    } catch (e) {
      // Not there: never again. Anything else keeps the last numbers.
      if (notThere(e)) {
        this.unsupported = true;
        this.set(null);
      }
    } finally {
      this.reading = false;
    }
  }

  private set(snap: ResourcesSnapshot | null) {
    this.snap = snap;
    for (const l of [...this.listeners]) l();
  }

  private start() {
    let timer: ReturnType<typeof setTimeout> | null = null;
    const schedule = () => {
      if (timer) clearTimeout(timer);
      timer = setTimeout(tick, hidden() ? RESOURCES_HIDDEN_MS : RESOURCES_VISIBLE_MS);
    };
    const tick = () => {
      void this.read();
      schedule();
    };
    tick();
    const onVisibility = () => {
      // Back in front: fresh numbers now, then the faster pace.
      if (!hidden()) tick();
      else schedule();
    };
    const doc = typeof document !== "undefined" ? document : null;
    doc?.addEventListener("visibilitychange", onVisibility);
    const unsubscribe = this.adapter.subscribe((e) => {
      if (e.type === "spaces.changed") void this.read();
    });
    this.stop = () => {
      if (timer) clearTimeout(timer);
      doc?.removeEventListener("visibilitychange", onVisibility);
      unsubscribe();
    };
  }
}

const stores = new WeakMap<DataAdapter, ResourcesStore>();

function storeFor(adapter: DataAdapter): ResourcesStore {
  let s = stores.get(adapter);
  if (!s) stores.set(adapter, (s = new ResourcesStore(adapter)));
  return s;
}

const none = () => () => {};
const nothing = () => null;

/** The latest numbers, or null (nothing to show). */
export function useResources(): ResourcesSnapshot | null {
  const adapter = useContext(BridgeContext)?.adapter ?? null;
  const store = adapter ? storeFor(adapter) : null;
  return useSyncExternalStore(store?.subscribe ?? none, store?.snapshot ?? nothing, nothing);
}

/** One computer's numbers, when it sent any: this Mac's, or another
 * computer's relay summary by its `machines.list` id. */
export function hostResources(snap: ResourcesSnapshot | null, machine: { id: string; current?: boolean } | undefined): ComputerResources | null {
  if (!machine || !snap) return null;
  return machine.current ? snap.thisMac : (snap.computers.find((c) => c.id === machine.id) ?? null);
}

/** One Space's numbers: one of this Mac's, or one of yours on another of
 * your computers (then only `diskOnHost`, on that computer, and its power
 * state are known). */
export function spaceResources(snap: ResourcesSnapshot | null, spaceId: string): SpaceResources | null {
  return snap?.spaces.find((s) => s.id === spaceId) ?? null;
}

/**
 * What a Space takes on its computer's disk, in bytes: what deleting it
 * frees, on this Mac or on the other computer of yours that runs it. Null
 * when that isn't known: a Space shared with you, one whose computer is
 * offline or hasn't answered yet, a container, or one whose disk shares
 * blocks with other copies so only an upper bound was measured.
 */
export function spaceDiskBytes(snap: ResourcesSnapshot | null, spaceId: string): number | null {
  return spaceResources(snap, spaceId)?.diskOnHost?.bytes ?? null;
}
