// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// A sign-in retried from the page (CUA-1728): the first attempt's browser
// came too late, "Open the browser again" started a second attempt, and the
// browser finished one of them. The app is signed in whichever it was, and
// a session stored while it waits ends the wait even when no wait returns.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SignInRuns, type SignInEnd, type SignInView } from "../src/model/sign-in";
import type { AccountProfile, AccountRunning, SignInAttempt, SignInOptions } from "../src/model/services";

/** Each sign-in is an attempt of its own (a loopback port), finished or failed by the test. */
class RetryAccount implements AccountRunning {
  stored: string | null = null;
  /** The options each start was given. */
  readonly starts: unknown[] = [];
  readonly attempts: { url: string; finish(who: string): void; fail(e: Error): void }[] = [];
  identity() {
    return this.stored;
  }
  profile(): AccountProfile | null {
    return null;
  }
  async beginSignIn(opts?: SignInOptions): Promise<SignInAttempt> {
    this.starts.push(opts);
    const url = `http://127.0.0.1:${50000 + this.attempts.length}/callback`;
    let finish!: (who: string) => void;
    let fail!: (e: Error) => void;
    const done = new Promise<string>((resolve, reject) => ((finish = resolve), (fail = reject)));
    this.attempts.push({
      url,
      finish: (who) => {
        // The SDK stores the session before the wait returns.
        this.stored = who;
        finish(who);
      },
      fail,
    });
    return { userCode: null, url, wait: () => done };
  }
  async signOut() {
    this.stored = null;
  }
  async accessToken() {
    return null;
  }
}

function view() {
  const seen = { phase: "idle", url: null as string | null, identity: null as string | null, failed: [] as SignInEnd[], signedIn: 0 };
  const v: SignInView = {
    starting: () => void (seen.phase = "starting"),
    waiting: (a) => void ((seen.phase = "waiting"), (seen.url = a.url)),
    signedIn: async (who) => void ((seen.phase = "idle"), (seen.identity = who), (seen.url = null), (seen.signedIn += 1)),
    failed: (m) => void ((seen.phase = "failed"), seen.failed.push(m)),
  };
  return { seen, v };
}

const flush = () => vi.advanceTimersByTimeAsync(0);

describe("a retried sign-in", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("starts with a code when the page asks for one (the browser never got back), then signs in", async () => {
    const account = new RetryAccount();
    const { seen, v } = view();
    const runs = new SignInRuns(() => account, v);
    void runs.begin();
    await flush();
    const byCode = runs.begin({ code: true });
    await flush();
    expect(account.starts).toEqual([undefined, { code: true }]);
    account.attempts[1]!.finish("ada@example.com");
    await byCode;
    expect(seen).toMatchObject({ phase: "idle", identity: "ada@example.com", signedIn: 1 });
  });

  it("finishes on the retry's attempt, and the first attempt's failure stays quiet", async () => {
    const account = new RetryAccount();
    const { seen, v } = view();
    const runs = new SignInRuns(() => account, v);
    const first = runs.begin();
    await flush();
    expect(seen).toMatchObject({ phase: "waiting", url: "http://127.0.0.1:50000/callback" });
    const retry = runs.begin();
    await flush();
    expect(seen.url).toBe("http://127.0.0.1:50001/callback");
    account.attempts[1]!.finish("ada@example.com");
    await retry;
    expect(seen).toMatchObject({ phase: "idle", identity: "ada@example.com", signedIn: 1 });
    // The first attempt's listener times out later: no failure shows.
    account.attempts[0]!.fail(new Error("browser sign-in was not completed in time"));
    await first;
    expect(seen).toMatchObject({ phase: "idle", failed: [], signedIn: 1 });
  });

  it("finishes when the browser completes the attempt the retry superseded", async () => {
    const account = new RetryAccount();
    const { seen, v } = view();
    const runs = new SignInRuns(() => account, v);
    const first = runs.begin();
    await flush();
    void runs.begin();
    await flush();
    account.attempts[0]!.finish("ada@example.com");
    await first;
    expect(seen).toMatchObject({ phase: "idle", identity: "ada@example.com", signedIn: 1 });
    // The retry's own wait ending afterwards changes nothing.
    account.attempts[1]!.fail(new Error("timed out"));
    await flush();
    expect(seen).toMatchObject({ phase: "idle", failed: [], signedIn: 1 });
  });

  it("ends the wait once a session is stored, even when no wait returns", async () => {
    const account = new RetryAccount();
    const { seen, v } = view();
    const runs = new SignInRuns(() => account, v);
    void runs.begin();
    await flush();
    void runs.begin();
    await flush();
    expect(seen.phase).toBe("waiting");
    // Signed in, but the attempt's wait is still busy (or never returns).
    account.stored = "ada@example.com";
    await vi.advanceTimersByTimeAsync(runs.watchMs);
    expect(seen).toMatchObject({ phase: "idle", identity: "ada@example.com", signedIn: 1 });
    await vi.advanceTimersByTimeAsync(runs.timeoutMs);
    expect(seen).toMatchObject({ phase: "idle", failed: [], signedIn: 1 });
  });

  it("still times out, once, when no attempt finishes", async () => {
    const account = new RetryAccount();
    const { seen, v } = view();
    const runs = new SignInRuns(() => account, v);
    void runs.begin();
    await flush();
    void runs.begin();
    await flush();
    await vi.advanceTimersByTimeAsync(runs.timeoutMs);
    expect(seen).toMatchObject({ phase: "failed", failed: [{ timedOut: true }], signedIn: 0 });
  });

  it("fails with the error it caught (the core reads its reason), never silently", async () => {
    const account = new RetryAccount();
    const { seen, v } = view();
    const runs = new SignInRuns(() => account, v);
    const run = runs.begin();
    await flush();
    const offline = new Error("could not reach the sign-in service: error sending request");
    account.attempts[0]!.fail(offline);
    await run;
    expect(seen).toMatchObject({ phase: "failed", failed: [{ error: offline }], signedIn: 0 });
  });
});
