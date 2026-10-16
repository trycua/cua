// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Each Space's latest thumbnail for the page (`spaces.thumbnail`): the same
 * image the SwiftUI app's notch tiles and preview cover draw
 * (`SpaceThumbnails`). The grid's tiles show it, and a Space's preview
 * shows it blurred and dimmed behind "Connecting…" or Connect.
 *
 * Memory is bounded like `SpaceThumbnails`: at most `THUMBNAIL_CAP` images,
 * the least recently used going first, and a Space that left the list
 * drops its image. A host with no thumbnails (`unsupported`) is not asked
 * again.
 */

import { useContext, useEffect, useSyncExternalStore } from "react";
import { isUnsupported, type DataAdapter } from "./adapter";
import { BridgeContext } from "./BridgeProvider";
import type { Space } from "./contracts/spaces";
import type { SpaceThumbnail } from "./ops/space-detail";
import type { BridgeStore } from "./store";

/** The most images held (thumbnails are about 320 px on the long edge). */
export const THUMBNAIL_CAP = 32;
/** How fresh a shown thumbnail is kept: the core's `thumbnail_policy`
 * background interval (a Space is asked again no sooner). */
export const THUMBNAIL_REFRESH_MS = 90_000;

export class ThumbnailStore {
  /** By Space id, least recently used first. */
  private entries = new Map<string, SpaceThumbnail>();
  private asked = new Map<string, number>();
  private inflight = new Set<string>();
  private unsupported = false;
  private listeners = new Set<() => void>();

  constructor(
    private readonly adapter: DataAdapter,
    private readonly now: () => number = Date.now,
    readonly cap = THUMBNAIL_CAP,
  ) {}

  subscribe = (l: () => void): (() => void) => {
    this.listeners.add(l);
    return () => this.listeners.delete(l);
  };

  /** The Space's latest image URL, or null while there is none. */
  get = (spaceId: string): string | null => this.entries.get(spaceId)?.url ?? null;

  /** Images held now. */
  get size(): number {
    return this.entries.size;
  }

  private publish(): void {
    for (const l of [...this.listeners]) l();
  }

  /** Keeps `t` as the Space's latest, unless a newer one is held. */
  set(spaceId: string, t: SpaceThumbnail): void {
    const held = this.entries.get(spaceId);
    if (held && held.capturedAtMs > t.capturedAtMs) return;
    this.entries.delete(spaceId);
    this.entries.set(spaceId, t);
    while (this.entries.size > this.cap) this.entries.delete(this.entries.keys().next().value!);
    this.publish();
  }

  /** Marks the Space's image as just used (it is evicted last). */
  touch(spaceId: string): void {
    const held = this.entries.get(spaceId);
    if (!held) return;
    this.entries.delete(spaceId);
    this.entries.set(spaceId, held);
  }

  /** Asks the host for the Space's thumbnail, unless it was asked within
   * `THUMBNAIL_REFRESH_MS` or is being asked now. */
  async request(spaceId: string): Promise<void> {
    if (this.unsupported || this.inflight.has(spaceId) || spaceId.startsWith("pending:")) return;
    const last = this.asked.get(spaceId);
    if (last !== undefined && this.now() - last < THUMBNAIL_REFRESH_MS) return;
    this.asked.set(spaceId, this.now());
    this.inflight.add(spaceId);
    try {
      const t = await this.adapter.call("spaces.thumbnail", { spaceId });
      if (t?.url) this.set(spaceId, t);
    } catch (e) {
      if (isUnsupported(e)) this.unsupported = true;
    } finally {
      this.inflight.delete(spaceId);
    }
  }

  /** Forgets the Spaces not in `ids` (deleted or no longer listed). */
  retain(ids: ReadonlySet<string>): void {
    let changed = false;
    for (const id of [...this.entries.keys()]) {
      if (!ids.has(id)) {
        this.entries.delete(id);
        changed = true;
      }
    }
    for (const id of [...this.asked.keys()]) if (!ids.has(id)) this.asked.delete(id);
    if (changed) this.publish();
  }
}

const stores = new WeakMap<BridgeStore, ThumbnailStore>();

/** The store's thumbnails, which follow its Spaces (a Space that leaves the
 * list drops its image). */
export function thumbnailStore(store: BridgeStore): ThumbnailStore {
  let t = stores.get(store);
  if (!t) {
    const made = new ThumbnailStore(store.adapter);
    let listed: Space[] | undefined;
    store.subscribe(() => {
      const spaces = store.get<Space[]>("spaces").data;
      if (!spaces || spaces === listed) return;
      listed = spaces;
      made.retain(new Set(spaces.map((s) => s.id)));
    });
    stores.set(store, made);
    t = made;
  }
  return t;
}

const noop = () => () => {};

/**
 * The Space's latest thumbnail while `active` (it can show one: it is
 * running, or was), asked again every `THUMBNAIL_REFRESH_MS` while mounted.
 * Null while there is none, or where the host has no thumbnails.
 */
export function useSpaceThumbnail(spaceId: string, active: boolean): string | null {
  const ctx = useContext(BridgeContext);
  const thumbs = ctx?.store ? thumbnailStore(ctx.store) : null;
  useEffect(() => {
    if (!thumbs || !active) return;
    void thumbs.request(spaceId);
    const timer = setInterval(() => void thumbs.request(spaceId), THUMBNAIL_REFRESH_MS);
    return () => clearInterval(timer);
  }, [thumbs, spaceId, active]);
  const url = useSyncExternalStore(thumbs ? thumbs.subscribe : noop, () => thumbs?.get(spaceId) ?? null, () => null);
  useEffect(() => {
    if (url) thumbs?.touch(spaceId);
  }, [thumbs, spaceId, url]);
  return url;
}
