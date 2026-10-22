// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The account's sign-in runs: "Sign in" starts an attempt (its own loopback
// port or device code), and starting again supersedes the attempt in
// progress. The app is signed in once a session is stored, whichever way it
// got there: the latest attempt's wait, an earlier attempt the browser
// finished after all, or another sign-in on this machine (`cua auth login`).
// The account's session is read again while a sign-in waits, so a session
// that appears ends the wait even when no attempt's wait returns.
import type { AccountRunning, SignInAttempt } from "./services";

/** Why a run ended without a session: the error it caught (the core reads
 * its case and words), or the browser never confirmed in time. */
export type SignInEnd = { error: unknown } | { timedOut: true };

/** What a run shows: the AppModel's phase and identity. */
export interface SignInView {
  starting(): void;
  waiting(attempt: SignInAttempt): void;
  signedIn(who: string | null): Promise<void>;
  failed(end: SignInEnd): void;
}

export class SignInRuns {
  /** How long an attempt waits for the browser (ms). */
  timeoutMs = 600_000;
  /** How often the stored session is read while a sign-in waits (ms). */
  watchMs = 2_000;
  /** The latest attempt; a retry, Cancel, a failure or a finish moves it on. */
  private attempt = 0;
  /** The attempt a sign-in last finished on: a later finish of an older attempt is the same session. */
  private finishedAt = 0;
  private watch: ReturnType<typeof setInterval> | null = null;

  constructor(
    private readonly account: () => AccountRunning | null,
    private readonly view: SignInView,
  ) {}

  /** Starts an attempt (superseding one in progress) and resolves once it ends. */
  async begin(): Promise<void> {
    const account = this.account();
    if (!account) return;
    const current = ++this.attempt;
    this.view.starting();
    this.watchStore(account, current);
    let attempt: SignInAttempt;
    try {
      attempt = await account.beginSignIn();
    } catch (error) {
      if (current === this.attempt) this.fail({ error });
      return;
    }
    // Cancelled, retried or signed in while it started.
    if (current !== this.attempt) return;
    this.view.waiting(attempt);
    const timeout = setTimeout(() => {
      if (current === this.attempt) this.fail({ timedOut: true });
    }, this.timeoutMs);
    timeout.unref?.();
    let who: string | null;
    try {
      who = await attempt.wait();
    } catch (error) {
      // An attempt a retry or Cancel left behind ends quietly.
      if (current === this.attempt) this.fail({ error });
      return;
    } finally {
      clearTimeout(timeout);
    }
    // Whichever attempt the browser finished stored the session: take it,
    // even when a retry superseded this attempt.
    await this.finish(current, who ?? account.identity());
  }

  /** Stops waiting (the attempt's own wait ends with its timeout). */
  cancel(): void {
    this.attempt += 1;
    this.stopWatch();
  }

  private async finish(started: number, who: string | null): Promise<void> {
    if (this.finishedAt >= started) return;
    this.finishedAt = ++this.attempt;
    this.stopWatch();
    try {
      await this.view.signedIn(who);
    } catch (error) {
      this.view.failed({ error });
    }
  }

  private fail(end: SignInEnd): void {
    this.attempt += 1;
    this.stopWatch();
    this.view.failed(end);
  }

  /** A session stored while the sign-in waits ends it. */
  private watchStore(account: AccountRunning, current: number): void {
    this.stopWatch();
    this.watch = setInterval(() => {
      if (current !== this.attempt) return this.stopWatch();
      const who = account.identity();
      if (who !== null) void this.finish(current, who);
    }, this.watchMs);
    this.watch.unref?.();
  }

  private stopWatch(): void {
    if (this.watch) clearInterval(this.watch);
    this.watch = null;
  }
}
