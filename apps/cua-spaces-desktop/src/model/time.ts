// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Bounded waits, as the SwiftUI app's `withTimeout`: an SDK call that never
// returns must not hold up the list poll, New Space or a reconnect. The call
// keeps running after the deadline; its result is dropped.

export class TimeoutError extends Error {
  constructor(seconds: number) {
    super(`timed out after ${seconds} s`);
    this.name = "TimeoutError";
  }
}

export type Outcome<T> = { ok: true; value: T } | { ok: false; error: unknown };

/** `body`'s result, or `TimeoutError` after `seconds`. */
export function withTimeout<T>(seconds: number, body: () => Promise<T> | T): Promise<Outcome<T>> {
  return new Promise((resolve) => {
    let done = false;
    const finish = (o: Outcome<T>) => {
      if (done) return;
      done = true;
      clearTimeout(timer);
      resolve(o);
    };
    const timer = setTimeout(() => finish({ ok: false, error: new TimeoutError(seconds) }), seconds * 1000);
    try {
      Promise.resolve(body()).then(
        (value) => finish({ ok: true, value }),
        (error: unknown) => finish({ ok: false, error }),
      );
    } catch (error) {
      finish({ ok: false, error });
    }
  });
}

/** The value, or null when it failed or timed out. */
export async function bounded<T>(seconds: number, body: () => Promise<T> | T): Promise<T | null> {
  const o = await withTimeout(seconds, body);
  return o.ok ? o.value : null;
}

export const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

export const nowMs = () => BigInt(Date.now());
