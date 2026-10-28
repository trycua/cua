// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * How the page polls the host so the load does not grow with what the
 * account owns. Before: the Agents tab read two thread calls for every Space
 * every 5 s (2 x 3,000 calls per 5 s at 3,000 Spaces), asked again every
 * time about Spaces with no thread, and every thumbnail was fetched the
 * moment its tile drew. Pure helpers (no timers), so they are tested
 * without a browser.
 */

/** The Agents tab's poll for `spaces` Spaces: 5 s up to 20 of them, then
 * 40 ms more for each, at most a minute. */
export function chatsPollMs(spaces: number): number {
  return Math.min(60_000, 5_000 + Math.max(0, spaces - 20) * 40);
}

/** `ms` spread by up to `fraction` either way, so a thousand pages (or a
 * thousand tabs after a reconnect) do not all ask in the same second. */
export function jittered(ms: number, fraction = 0.2, random: () => number = Math.random): number {
  return Math.max(0, Math.round(ms * (1 + (random() * 2 - 1) * fraction)));
}

/** `f` over `items`, at most `limit` at a time, answers in order. */
export async function mapLimit<T, R>(items: readonly T[], limit: number, f: (item: T, index: number) => Promise<R>): Promise<R[]> {
  const out = new Array<R>(items.length);
  let next = 0;
  const worker = async () => {
    for (let i = next++; i < items.length; i = next++) out[i] = await f(items[i]!, i);
  };
  await Promise.all(Array.from({ length: Math.min(Math.max(1, limit), items.length) }, worker));
  return out;
}

/** Ids that failed in a way that will not change soon (a Space with no
 * thread: a stopped machine, an old daemon), not asked again for `ttlMs`. */
export class MissCache {
  private until = new Map<string, number>();
  constructor(
    private readonly ttlMs: number,
    private readonly now: () => number = Date.now,
  ) {}

  add(id: string): void {
    this.until.set(id, this.now() + this.ttlMs);
  }

  has(id: string): boolean {
    const t = this.until.get(id);
    if (t === undefined) return false;
    if (t <= this.now()) {
      this.until.delete(id);
      return false;
    }
    return true;
  }

  /** Forgets ids not in `keep` (Spaces that left the list). */
  prune(keep: ReadonlySet<string>): void {
    for (const id of [...this.until.keys()]) if (!keep.has(id)) this.until.delete(id);
  }

  get size(): number {
    return this.until.size;
  }
}

/** A queue that runs at most `limit` jobs at once, in the order they came
 * (thumbnails: a screenful of tiles drawing at once is a few calls at a time,
 * not one per tile). */
export class JobQueue {
  private running = 0;
  private waiting: Array<() => void> = [];
  constructor(private readonly limit: number) {}

  async run<T>(job: () => Promise<T>): Promise<T> {
    if (this.running >= this.limit) await new Promise<void>((resolve) => this.waiting.push(resolve));
    this.running++;
    try {
      return await job();
    } finally {
      this.running--;
      this.waiting.shift()?.();
    }
  }

  get active(): number {
    return this.running;
  }

  get queued(): number {
    return this.waiting.length;
  }
}
