// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The app's in-memory layer over the SDK's thumbnail cache (the SwiftUI app's
// SpaceThumbnails.swift): the latest image per Space, so the notch tiles and
// the page's previews paint at once. The cua daemon's cache is shared by
// every client on this computer; nothing here captures a screen itself.
//
// - `refresh(id, maxAgeSec)` asks the SDK for an image no older than that
//   (the daemon answers from its cache, or captures).
// - `warm(ids)` fills Spaces with no image yet from the daemon's cache (any
//   age), once each per launch, so a preview exists right after launch.
// - `keepFresh` asks again for every running Space every background
//   interval while the app is in use, which keeps the daemon's own
//   background refresh going.
import type { SpaceThumbnailData } from "./backend";

export interface ThumbnailEntry {
  image: Uint8Array;
  format: "png" | "jpeg";
  capturedAtMs: number;
  /** The decoded pixels' size in bytes (what the cap counts). */
  bytes: number;
}

/** The core's `thumbnail_policy`, in the units used here. */
export interface ThumbnailPolicy {
  openIntervalMs: number;
  backgroundIntervalMs: number;
  maxDimension: number;
}

export type ThumbnailFetch = (id: string, maxAgeMs: number | null) => Promise<SpaceThumbnailData | null>;

export class SpaceThumbnails {
  static readonly defaultByteCap = 64 << 20;
  /** The most bytes of images held; past it the least recently used Space's image goes first. */
  byteCap = SpaceThumbnails.defaultByteCap;
  /** Bytes held now. */
  totalBytes = 0;
  /** By Space id, least recently used first. */
  private readonly entries = new Map<string, ThumbnailEntry>();
  private readonly inFlight = new Map<string, Promise<ThumbnailEntry | null>>();
  private readonly warmed = new Set<string>();
  private keeping: (() => void) | null = null;
  private readonly listeners = new Set<(id: string) => void>();

  constructor(
    readonly policy: ThumbnailPolicy,
    /** The SDK call (`Space.thumbnail(maxAgeMs)`); none: memory only. */
    public fetch: ThumbnailFetch | null = null,
  ) {}

  /** Called with a Space's id when its image changed (or went). */
  subscribe(listener: (id: string) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  private changed(id: string): void {
    for (const l of [...this.listeners]) l(id);
  }

  /** The Space's latest image (marks it as just used), or null. */
  get(id: string): ThumbnailEntry | null {
    const entry = this.entries.get(id);
    if (!entry) return null;
    this.entries.delete(id);
    this.entries.set(id, entry);
    return entry;
  }

  get size(): number {
    return this.entries.size;
  }

  /** Keeps `t` as the Space's latest, unless a newer one is held. */
  set(id: string, t: SpaceThumbnailData): void {
    const held = this.entries.get(id);
    if (held && held.capturedAtMs > t.capturedAtMs) return;
    const bytes = t.width > 0 && t.height > 0 ? t.width * t.height * 4 : t.image.byteLength;
    this.totalBytes += bytes - (held?.bytes ?? 0);
    this.entries.delete(id);
    this.entries.set(id, { image: t.image, format: t.format, capturedAtMs: t.capturedAtMs, bytes });
    this.evict(id);
    this.changed(id);
  }

  remove(id: string): void {
    const held = this.entries.get(id);
    if (!held) return;
    this.entries.delete(id);
    this.totalBytes -= held.bytes;
    this.changed(id);
  }

  /** Forgets the Spaces not in `ids` (deleted or forgotten). */
  retain(ids: ReadonlySet<string>): void {
    for (const id of [...this.entries.keys()]) if (!ids.has(id)) this.remove(id);
  }

  /** Drops the least recently used images until the rest fit the cap (`keeping`'s own stays even alone over it). */
  private evict(keeping: string): void {
    for (const id of [...this.entries.keys()]) {
      if (this.totalBytes <= this.byteCap) return;
      if (id !== keeping) this.remove(id);
    }
  }

  /** Asks the SDK for the Space's image no older than `maxAgeSec` (null: any age) and keeps it; the latest image. */
  async refresh(id: string, maxAgeSec: number | null): Promise<ThumbnailEntry | null> {
    const fetch = this.fetch;
    if (!fetch) return this.get(id);
    const running = this.inFlight.get(id);
    if (running) return running;
    const task = (async () => {
      try {
        const t = await fetch(id, maxAgeSec === null ? null : Math.max(0, maxAgeSec) * 1000);
        if (t) this.set(id, t);
      } catch {
        // No image this time: keep what is held.
      }
      return this.get(id);
    })();
    this.inFlight.set(id, task);
    try {
      return await task;
    } finally {
      this.inFlight.delete(id);
    }
  }

  /** Fills each Space in `ids` that has no image yet from the daemon's cache (any age), once per launch. */
  async warm(ids: readonly string[]): Promise<void> {
    const missing = ids.filter((id) => !this.entries.has(id) && !this.warmed.has(id));
    for (const id of missing) this.warmed.add(id);
    await Promise.all(missing.map((id) => this.refresh(id, null)));
  }

  /**
   * While the app runs: every running Space gets an image no older than the
   * policy's background interval, every interval, skipped while `active`
   * says the app is not in use. Returns the stop.
   */
  keepFresh(running: () => readonly string[], active: () => boolean = () => true, sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms).unref?.())): () => void {
    this.keeping?.();
    let stopped = false;
    const interval = this.policy.backgroundIntervalMs / 1000;
    void (async () => {
      while (!stopped) {
        if (active()) for (const id of running()) await this.refresh(id, interval);
        await sleep(this.policy.backgroundIntervalMs);
      }
    })();
    const stop = () => {
      stopped = true;
    };
    this.keeping = stop;
    return stop;
  }

  stop(): void {
    this.keeping?.();
    this.keeping = null;
  }
}

/** A thumbnail as a `data:` URL the page can draw. */
export function thumbnailDataUrl(entry: Pick<ThumbnailEntry, "image" | "format">): string {
  return `data:image/${entry.format};base64,${Buffer.from(entry.image.buffer, entry.image.byteOffset, entry.image.byteLength).toString("base64")}`;
}
