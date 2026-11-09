// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Runs the preparing state machine (`prepare.rs` in the app core) against
 * the host: it holds the state, saves it after every step so a quit loses
 * nothing, and does what the core's effects say (make the home Space, open
 * the agent's session, post a message, look in the thread). Every decision
 * is the core's; this file only does the work and reports what happened.
 *
 * Nothing here reads the clock or the host directly: both come in through
 * [`PrepareEnv`], so the driver is tested with a fake host, a fake clock and
 * a connection that drops.
 */

import type { CoreClient } from "@/bridge/core";
import type { PrepareAction, PrepareEffect, PrepareMessage, PrepareMetrics, PrepareState, PrepareView } from "@/bridge/contracts/prepare";
import { prepareMetrics, prepareReduce, prepareView } from "@/bridge/prepare-core";

/** Where the state is kept between launches. */
export interface PrepareStorage {
  load(): PrepareState | null;
  save(state: PrepareState): void;
}

/** What the driver needs from the host. */
export interface PrepareEnv {
  now(): number;
  /** The machine has a connection (the browser's reading). */
  online(): boolean;
  /** A new id for the home's row while it is made. */
  newPendingId(): string;
  /** The home Space for this coding app, when one exists (`creating`: the host is still making it, started before this page loaded). */
  findHome(harness: string): { id: string; running: boolean; creating?: boolean } | null;
  /** Waits for a create the host is running (started before this page loaded) to end; rejects when it fails. */
  followCreate(id: string): Promise<void>;
  /** Turns a stopped home back on. */
  startHome(id: string): Promise<void>;
  /** Makes the home and resolves with its id once it is running. Carries on a create that is already there. */
  createHome(a: { harness: string; pendingId: string }): Promise<string>;
  /** Puts the coding app in the home and resolves once its session is open. */
  attachAgent(a: { spaceId: string; harness: string; model: string | null }): Promise<void>;
  /** Posts a message to the home's thread, as the box does. */
  post(a: { spaceId: string; message: PrepareMessage }): Promise<void>;
  /** Whether the thread already has this message (by its text, from the person, after `sinceMs`). */
  threadHas(a: { spaceId: string; message: PrepareMessage }): Promise<boolean>;
  /** Whether the agent has written since `sinceMs`. */
  agentReplied(a: { spaceId: string; sinceMs: number }): Promise<boolean>;
  /** The home is gone from the list (null: the list is not loaded yet). */
  homeExists(id: string): boolean | null;
  /** A message reached its Space: the app's own record of the task (the Home list) follows. */
  delivered?(m: PrepareMessage, spaceId: string): void;
}

/** Words of a failure that is the connection's, not the machine's. */
const NETWORK =
  /(network|connection|connect|timed?\s?out|time-?out|offline|dns|resolve|resolving|reset|refused|unreachable|temporary failure|tls|handshake|eof|broken pipe|no route|download|registry|ghcr|socket|try again later|\b50[234]\b)/i;

/** Whether a failure is about the connection (it carries on by itself). */
export function isNetworkFailure(message: string): boolean {
  return NETWORK.test(message);
}

const text = (e: unknown) => (e instanceof Error ? e.message : String(e));

export class PrepareDriver {
  private s: PrepareState | null;
  private listeners = new Set<() => void>();
  private creating = false;
  private attaching = false;
  private readonly inflight = new Set<string>();
  private watching = false;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private v: PrepareView | null = null;

  constructor(
    private readonly core: CoreClient,
    private env: PrepareEnv,
    private readonly store: PrepareStorage,
  ) {
    this.s = store.load();
    this.v = prepareView(core, this.s);
  }

  /** The host's latest closures (the page re-renders with new ones). */
  setEnv(env: PrepareEnv): void {
    this.env = env;
  }

  get state(): PrepareState | null {
    return this.s;
  }

  view(): PrepareView | null {
    return this.v;
  }

  /** The page as a property (a new object each step), for React: a method call would be cached by the compiler. */
  get pageView(): PrepareView | null {
    return this.v;
  }

  metrics(): PrepareMetrics | null {
    return prepareMetrics(this.core, this.s);
  }

  subscribe = (l: () => void): (() => void) => {
    this.listeners.add(l);
    return () => this.listeners.delete(l);
  };

  private emit(): void {
    for (const l of [...this.listeners]) l();
  }

  /** One action through the core; the state is saved, then the effects run. */
  dispatch(action: PrepareAction): void {
    const r = prepareReduce(this.core, this.s, action);
    if (!r) return;
    this.s = r.state;
    this.v = prepareView(this.core, this.s);
    this.store.save(r.state);
    this.emit();
    for (const e of r.effects) this.run(e);
  }

  /** Begins preparing for `harness`; asking again changes nothing. */
  start(harness: string): void {
    if (this.s?.wanted) return;
    this.dispatch({ type: "start", nowMs: this.env.now(), harness, pendingId: this.env.newPendingId() });
    if (!this.env.online()) this.dispatch({ type: "online", nowMs: this.env.now(), online: false });
  }

  /** The app started again with a saved record: whatever was in flight is looked at again. */
  resume(): void {
    if (!this.s?.wanted) return;
    this.dispatch({ type: "resume", nowMs: this.env.now() });
    if (!this.env.online()) this.dispatch({ type: "online", nowMs: this.env.now(), online: false });
    this.watchReplies();
  }

  /** The person sent a message: kept, then delivered when ready, exactly once. */
  send(m: { id: string; text: string; agent: string; model?: string | null; repo?: string | null }): void {
    this.dispatch({ type: "send", nowMs: this.env.now(), id: m.id, text: m.text, agent: m.agent, model: m.model ?? null, repo: m.repo ?? null });
  }

  retry(): void {
    this.dispatch({ type: "retry", nowMs: this.env.now() });
  }

  /** A person wrote into the home's own thread: from now on it is listed like any Space. */
  asked(): void {
    if (this.s?.wanted && this.s.askedMs == null) this.dispatch({ type: "asked", nowMs: this.env.now() });
  }

  onboardingDone(): void {
    this.dispatch({ type: "onboardingDone", nowMs: this.env.now() });
  }

  online(online: boolean): void {
    if (this.s && this.s.online === online) return;
    this.dispatch({ type: "online", nowMs: this.env.now(), online });
  }

  /** A create progress report for the home's row. */
  progress(p: { phase: string; detail?: string; bytesDone?: number | null; bytesTotal?: number | null; bytesPerSecond?: number | null }): void {
    if (!this.s?.wanted || this.s.homeSpaceId) return;
    this.dispatch({
      type: "phase",
      nowMs: this.env.now(),
      phase: p.phase,
      detail: p.detail ?? "",
      done: p.bytesDone ?? null,
      total: p.bytesTotal ?? null,
      perSecond: p.bytesPerSecond != null ? Math.round(p.bytesPerSecond) : null,
    });
  }

  /** The list of Spaces changed: a ready home that is gone is made again. */
  checkHome(): void {
    const s = this.s;
    if (!s?.wanted || !s.homeSpaceId || !s.readyMs) return;
    if (this.env.homeExists(s.homeSpaceId) === false) this.dispatch({ type: "homeGone", nowMs: this.env.now() });
  }

  /** About once a second while something waits: time passes. */
  tick(): void {
    if (this.s?.wanted) this.dispatch({ type: "tick", nowMs: this.env.now() });
  }

  private run(e: PrepareEffect): void {
    switch (e.type) {
      case "create":
        void this.create();
        break;
      case "attachAgent":
        void this.attach();
        break;
      case "deliver":
        void this.deliver(e.id);
        break;
      case "verifyDelivery":
        void this.verify(e.id);
        break;
      case "wakeAt": {
        if (this.timer) clearTimeout(this.timer);
        const wait = Math.max(0, e.atMs - this.env.now());
        this.timer = setTimeout(() => {
          this.timer = null;
          this.tick();
        }, wait);
        break;
      }
    }
  }

  private async create(): Promise<void> {
    if (this.creating) return;
    this.creating = true;
    try {
      const s = this.s!;
      const found = this.env.findHome(s.harness);
      if (found) {
        // A create that outlived the app (the host keeps running it): follow it, never start a second one.
        if (found.creating) await this.env.followCreate(found.id);
        else if (!found.running) await this.env.startHome(found.id);
        this.dispatch({ type: "homeReady", nowMs: this.env.now(), spaceId: found.id });
        return;
      }
      const id = await this.env.createHome({ harness: s.harness, pendingId: s.pendingId ?? this.env.newPendingId() });
      this.dispatch({ type: "homeReady", nowMs: this.env.now(), spaceId: id });
    } catch (e) {
      const message = text(e);
      this.dispatch({ type: "failed", nowMs: this.env.now(), message, network: isNetworkFailure(message) || !this.env.online() });
    } finally {
      this.creating = false;
    }
  }

  private async attach(): Promise<void> {
    if (this.attaching) return;
    this.attaching = true;
    try {
      const s = this.s!;
      const first = s.messages.find((m) => m.state !== "delivered");
      await this.env.attachAgent({ spaceId: s.homeSpaceId!, harness: s.harness, model: first?.model ?? null });
      this.dispatch({ type: "agentReady", nowMs: this.env.now() });
      this.watchReplies();
    } catch (e) {
      const message = text(e);
      this.dispatch({ type: "failed", nowMs: this.env.now(), message, network: isNetworkFailure(message) || !this.env.online() });
    } finally {
      this.attaching = false;
    }
  }

  private message(id: string): PrepareMessage | undefined {
    return this.s?.messages.find((m) => m.id === id);
  }

  private async deliver(id: string): Promise<void> {
    if (this.inflight.has(id)) return;
    this.inflight.add(id);
    try {
      const m = this.message(id);
      const space = this.s?.homeSpaceId;
      if (!m || !space) return;
      await this.env.post({ spaceId: space, message: m });
      this.dispatch({ type: "delivered", nowMs: this.env.now(), id });
      this.env.delivered?.(this.message(id) ?? m, space);
      this.watchReplies();
    } catch {
      this.dispatch({ type: "deliveryFailed", nowMs: this.env.now(), id });
    } finally {
      this.inflight.delete(id);
    }
  }

  private async verify(id: string): Promise<void> {
    if (this.inflight.has(id)) return;
    this.inflight.add(id);
    try {
      const m = this.message(id);
      const space = this.s?.homeSpaceId;
      if (!m || !space) return;
      let found = false;
      try {
        found = await this.env.threadHas({ spaceId: space, message: m });
      } catch {
        // Cannot tell: the thread is not asked again blindly; try the check once more soon.
        this.timer = setTimeout(() => {
          this.timer = null;
          this.inflight.delete(id);
          void this.verify(id);
        }, 2000);
        return;
      }
      // The answer can ask for this very message again (not found): it is no longer in flight.
      this.inflight.delete(id);
      this.dispatch({ type: "threadHas", nowMs: this.env.now(), id, found });
      if (found) this.env.delivered?.(this.message(id) ?? m, space);
    } finally {
      this.inflight.delete(id);
    }
  }

  /** Records when the agent first answers each delivered message (for the numbers); stops when none waits. */
  private watchReplies(): void {
    if (this.watching) return;
    this.watching = true;
    const started = this.env.now();
    const step = async () => {
      const waiting = this.s?.messages.filter((m) => m.state === "delivered" && m.firstTokenMs == null && m.deliveredMs != null) ?? [];
      const space = this.s?.homeSpaceId;
      if (!space || this.env.now() - started > 10 * 60_000) {
        this.watching = false;
        return;
      }
      for (const m of waiting) {
        try {
          if (await this.env.agentReplied({ spaceId: space, sinceMs: m.deliveredMs! })) this.dispatch({ type: "firstToken", nowMs: this.env.now(), id: m.id });
        } catch {
          // Asked again next round.
        }
      }
      const more = this.s?.messages.some((m) => m.state !== "delivered" || (m.firstTokenMs == null && m.deliveredMs != null));
      if (more) setTimeout(() => void step(), 700);
      else this.watching = false;
    };
    void step();
  }
}

/** The state kept in `localStorage` (a failing store is only a lost resume). */
export function localPrepareStorage(key = "cua.prepare.v1"): PrepareStorage {
  return {
    load() {
      try {
        const raw = globalThis.localStorage?.getItem(key);
        return raw ? (JSON.parse(raw) as PrepareState) : null;
      } catch {
        return null;
      }
    },
    save(state) {
      try {
        globalThis.localStorage?.setItem(key, JSON.stringify(state));
      } catch {
        // Kept for this run only.
      }
    },
  };
}
