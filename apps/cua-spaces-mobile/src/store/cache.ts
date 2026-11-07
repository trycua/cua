// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Cached threads and the last Spaces list, sealed on disk (see `sealed.ts`), so the app opens on what it last knew
 * and keeps showing it, labelled as saved, while the machines cannot be reached. Nothing here is ever shown as
 * current: a cached read carries the time it was saved, and the line over it (the app core's `cache.line`) says so.
 */

import type { AgentMember, SpaceList, ThreadEntry } from "../data/types";
import type { SealedStore } from "./sealed";

const THREAD_ENTRIES = 200;
const MAX_THREADS = 40;
const SPACES = "spaces";
const THREADS_INDEX = "threads";

export interface CachedThread {
  entries: ThreadEntry[];
  members: AgentMember[];
  me: string | null;
  role: string | null;
  readSeq: number | null;
  savedMs: number;
}

export interface CachedSpaces {
  list: SpaceList;
  savedMs: number;
}

const slotOf = (spaceId: string) => `thread:${spaceId}`;

export class Cache {
  private pending = new Map<string, { handle: ReturnType<typeof setTimeout>; thread: CachedThread }>();

  constructor(
    private readonly store: SealedStore,
    private readonly now: () => number = Date.now,
    /** How long a changing thread waits before it is written again. */
    private readonly delayMs = 1_500,
  ) {}

  get available(): boolean {
    return this.store.available;
  }

  async loadThread(spaceId: string): Promise<CachedThread | null> {
    const raw = await this.store.get(slotOf(spaceId));
    if (!raw) return null;
    try {
      const t = JSON.parse(raw) as CachedThread;
      return Array.isArray(t.entries) ? t : null;
    } catch {
      return null;
    }
  }

  /** Writes soon (a streaming thread changes many times a second); the newest state wins. */
  saveThread(spaceId: string, t: Omit<CachedThread, "savedMs">): void {
    if (!this.store.available) return;
    const prev = this.pending.get(spaceId);
    if (prev) clearTimeout(prev.handle);
    const thread: CachedThread = { ...t, entries: t.entries.slice(-THREAD_ENTRIES), savedMs: this.now() };
    const handle = setTimeout(() => {
      this.pending.delete(spaceId);
      void this.writeThread(spaceId, thread);
    }, this.delayMs);
    this.pending.set(spaceId, { handle, thread });
  }

  /** Writes now (the app is going to the background). */
  async flush(): Promise<void> {
    const all = [...this.pending.entries()];
    this.pending.clear();
    for (const [id, p] of all) {
      clearTimeout(p.handle);
      await this.writeThread(id, p.thread);
    }
  }

  private async writeThread(spaceId: string, t: CachedThread): Promise<void> {
    await this.store.set(slotOf(spaceId), JSON.stringify(t));
    // Keep the most recently saved threads; drop the oldest beyond a few dozen.
    try {
      const raw = await this.store.get(THREADS_INDEX);
      const index: { id: string; ms: number }[] = raw ? JSON.parse(raw) : [];
      const next = [{ id: spaceId, ms: t.savedMs }, ...index.filter((i) => i.id !== spaceId)];
      for (const gone of next.slice(MAX_THREADS)) await this.store.remove(slotOf(gone.id));
      await this.store.set(THREADS_INDEX, JSON.stringify(next.slice(0, MAX_THREADS)));
    } catch {
      /* the index is a convenience */
    }
  }

  async loadSpaces(): Promise<CachedSpaces | null> {
    const raw = await this.store.get(SPACES);
    if (!raw) return null;
    try {
      const c = JSON.parse(raw) as CachedSpaces;
      return c && c.list && Array.isArray(c.list.spaces) ? c : null;
    } catch {
      return null;
    }
  }

  async saveSpaces(list: SpaceList): Promise<void> {
    await this.store.set(SPACES, JSON.stringify({ list, savedMs: this.now() } satisfies CachedSpaces));
  }

  /** Sign-out. */
  async clear(): Promise<void> {
    for (const p of this.pending.values()) clearTimeout(p.handle);
    this.pending.clear();
    await this.store.clear();
  }
}
