// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { chatsPollMs, forEachLimit, jittered, JobQueue, mapLimit, MissCache } from "./poll-plan";

describe("chatsPollMs", () => {
  it("is 5 s for a small account and slows as it grows, to a minute", () => {
    expect(chatsPollMs(0)).toBe(5_000);
    expect(chatsPollMs(20)).toBe(5_000);
    expect(chatsPollMs(120)).toBe(5_000 + 100 * 40);
    expect(chatsPollMs(3_000)).toBe(60_000);
    expect(chatsPollMs(1_000_000)).toBe(60_000);
  });

  it("keeps the calls per second flat instead of growing with the Spaces", () => {
    // Two calls per Space per poll: calls per second.
    const rate = (n: number) => (2 * n) / (chatsPollMs(n) / 1000);
    expect(rate(20)).toBeCloseTo(8);
    expect(rate(3_000)).toBeLessThan(105); // was 1,200 a second at a flat 5 s
    expect(rate(3_000)).toBeLessThan(rate(20) * 13);
  });
});

describe("jittered", () => {
  it("spreads a delay by the fraction either way", () => {
    expect(jittered(10_000, 0.2, () => 0)).toBe(8_000);
    expect(jittered(10_000, 0.2, () => 1)).toBe(12_000);
    expect(jittered(10_000, 0.2, () => 0.5)).toBe(10_000);
  });
  it("never goes negative", () => {
    expect(jittered(5, 5, () => 0)).toBe(0);
  });
});

describe("mapLimit", () => {
  it("answers in order and never runs more than the limit at once", async () => {
    let live = 0;
    let peak = 0;
    const out = await mapLimit([1, 2, 3, 4, 5, 6, 7, 8, 9, 10], 3, async (n) => {
      live++;
      peak = Math.max(peak, live);
      await new Promise((r) => setTimeout(r, 5));
      live--;
      return n * 2;
    });
    expect(out).toEqual([2, 4, 6, 8, 10, 12, 14, 16, 18, 20]);
    expect(peak).toBe(3);
  });
  it("handles an empty list and a limit larger than the list", async () => {
    expect(await mapLimit([], 4, async () => 1)).toEqual([]);
    expect(await mapLimit([1, 2], 50, async (n) => n)).toEqual([1, 2]);
  });
});

describe("MissCache", () => {
  it("leaves an id alone until its time is up", () => {
    let now = 1_000;
    const c = new MissCache(60_000, () => now);
    expect(c.has("a")).toBe(false);
    c.add("a");
    expect(c.has("a")).toBe(true);
    now += 59_999;
    expect(c.has("a")).toBe(true);
    now += 2;
    expect(c.has("a")).toBe(false);
    expect(c.size).toBe(0);
  });
  it("forgets Spaces that left the list", () => {
    const c = new MissCache(60_000, () => 0);
    c.add("a");
    c.add("b");
    c.prune(new Set(["b"]));
    expect(c.has("a")).toBe(false);
    expect(c.has("b")).toBe(true);
  });
});

describe("JobQueue", () => {
  it("runs a few jobs at a time and the rest in the order they came", async () => {
    const q = new JobQueue(2);
    const order: number[] = [];
    let peak = 0;
    await Promise.all(
      [1, 2, 3, 4, 5].map((n) =>
        q.run(async () => {
          peak = Math.max(peak, q.active);
          order.push(n);
          await new Promise((r) => setTimeout(r, 5));
        }),
      ),
    );
    expect(order).toEqual([1, 2, 3, 4, 5]);
    expect(peak).toBe(2);
    expect(q.active).toBe(0);
    expect(q.queued).toBe(0);
  });
  it("lets the next job start when one fails", async () => {
    const q = new JobQueue(1);
    const failed = q.run(async () => {
      throw new Error("x");
    });
    const next = q.run(async () => "ok");
    await expect(failed).rejects.toThrow("x");
    await expect(next).resolves.toBe("ok");
  });
});

describe("forEachLimit", () => {
  it("runs at most `limit` at once and starts every item", async () => {
    let active = 0;
    let peak = 0;
    const done: number[] = [];
    await forEachLimit([1, 2, 3, 4, 5, 6], 2, 1000, async (n) => {
      active++;
      peak = Math.max(peak, active);
      await new Promise((r) => setTimeout(r, 5));
      active--;
      done.push(n);
    });
    expect(peak).toBe(2);
    expect(done.sort()).toEqual([1, 2, 3, 4, 5, 6]);
  });

  it("a hung item holds its slot for the slot time only, never the rest of the list", async () => {
    const started: number[] = [];
    const t0 = Date.now();
    await forEachLimit([1, 2, 3, 4], 1, 30, async (n) => {
      started.push(n);
      if (n === 1) await new Promise(() => undefined);
    });
    // One slot, the first never answers: the others still ran, after its 30 ms.
    expect(started).toEqual([1, 2, 3, 4]);
    expect(Date.now() - t0).toBeLessThan(1000);
  });

  it("a late result still lands: the job keeps running after its slot is freed", async () => {
    let finished = false;
    await forEachLimit([1], 1, 10, async () => {
      await new Promise((r) => setTimeout(r, 40));
      finished = true;
    });
    expect(finished).toBe(false);
    await new Promise((r) => setTimeout(r, 60));
    expect(finished).toBe(true);
  });

  it("a failing job does not stop the others", async () => {
    const ran: number[] = [];
    await forEachLimit([1, 2, 3], 3, 100, async (n) => {
      ran.push(n);
      if (n === 2) throw new Error("no");
    });
    expect(ran).toEqual([1, 2, 3]);
  });
});
