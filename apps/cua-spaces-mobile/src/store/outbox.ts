// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The outbox driver: writes down what the person sends before anything is posted, keeps it sealed on disk, and does
 * what the app core's outbox state machine says (`outbox.reduce`, `libs/cua/crates/cua-spaces-app-core/src/phone/outbox.rs`):
 * post a claimed message, look for one found after a quit, wake for a retry. The rules (exactly once, in order per
 * Space, back off on a lost connection, give up loudly after five real failures) are the core's, the same ones the
 * Mac and web apps use to keep an onboarding message; this file runs the clock and the network.
 *
 * Where the core is not linked there is no state machine to ask: a send then goes straight to the backend and a failure
 * is the caller's (no queue is faked).
 */

import * as Crypto from "expo-crypto";

import { core } from "../core";
import {
  outboxFound,
  outboxInitial,
  outboxReduce,
  outboxView,
  type EntryLite,
  type OutboxAction,
  type OutboxEffect,
  type OutboxState,
  type OutboxView,
  type OutMessage,
} from "../core/phone";
import type { Backend, TaskResult, ThreadEntry } from "../data/types";
import { errorText, isNetworkError } from "./network";
import type { SealedStore } from "./sealed";

const SLOT = "outbox";

export interface SendInput {
  space: string;
  kind?: "post" | "answer" | "task";
  text?: string;
  replyTo?: number | null;
  mode?: "send" | "queue" | "steer";
  /** What posting needs beyond the text: the view, the steered agent, the answer, the task request. */
  extra?: Record<string, unknown> | null;
}

export interface Delivered {
  id: string;
  space: string;
  kind: string;
  entry: ThreadEntry | null;
  task: TaskResult | null;
}

export interface OutboxDeps {
  store: SealedStore;
  backend(): Backend | null;
  me(): string | null;
  now?(): number;
  newId?(): string;
}

const EMPTY_VIEW: OutboxView = { rows: [], banner: null, waiting: 0, failed: 0 };

export const randomId = (): string => {
  try {
    const id = Crypto.randomUUID?.();
    if (id) return id;
  } catch {
    /* fall through */
  }
  return `m${Date.now().toString(36)}${Math.random().toString(36).slice(2, 10)}`;
};

export class OutboxDriver {
  private state: OutboxState | null = outboxInitial();
  private version = 0;
  private readonly listeners = new Set<() => void>();
  private readonly deliveredListeners = new Set<(d: Delivered) => void>();
  private readonly waiters = new Map<string, { resolve(d: Delivered): void; reject(e: Error): void }>();
  private readonly results = new Map<string, Delivered>();
  private persisting: Promise<void> = Promise.resolve();
  private timer: ReturnType<typeof setTimeout> | null = null;
  private timerAt = 0;
  private readonly verifying = new Set<string>();
  readonly ready: Promise<void>;
  private readonly now: () => number;
  private readonly newId: () => string;

  constructor(private readonly deps: OutboxDeps) {
    this.now = deps.now ?? Date.now;
    this.newId = deps.newId ?? randomId;
    this.ready = this.load();
  }

  /* ---- reading ---- */

  subscribe = (fn: () => void): (() => void) => {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  };
  getVersion = (): number => this.version;

  /** What the thread (or Home, with no Space) draws. */
  view(space?: string | null): OutboxView {
    if (!this.state) return EMPTY_VIEW;
    return outboxView(this.state, space) ?? EMPTY_VIEW;
  }

  message(id: string): OutMessage | undefined {
    return this.state?.messages.find((m) => m.id === id);
  }

  /** Messages that wait, sending or failed, for a Space (for the composer's "1 queued" and Home's count). */
  pendingCount(space?: string | null): number {
    return this.state ? this.state.messages.filter((m) => m.state !== "delivered" && (!space || m.space === space)).length : 0;
  }

  /** Called with each message the daemon took, so the thread shows its entry at once. */
  onDelivered(fn: (d: Delivered) => void): () => void {
    this.deliveredListeners.add(fn);
    return () => this.deliveredListeners.delete(fn);
  }

  /* ---- sending ---- */

  /** Writes it down and sends it; `done` settles when the daemon has it (or it gave up). */
  async send(input: SendInput): Promise<{ id: string; done: Promise<Delivered> }> {
    await this.ready;
    const id = this.newId();
    const kind = input.kind ?? "post";
    const done = new Promise<Delivered>((resolve, reject) => this.waiters.set(id, { resolve, reject }));
    // A caller that never awaits `done` must not raise an unhandled rejection when the message is later deleted.
    done.catch(() => {});
    if (!this.state) {
      // No state machine: straight to the backend.
      void this.deliverDirect(id, { ...input, kind });
      return { id, done };
    }
    this.apply({
      type: "send",
      nowMs: this.now(),
      id,
      space: input.space,
      kind,
      text: input.text ?? "",
      replyTo: input.replyTo ?? null,
      mode: input.mode ?? "send",
      extra: input.extra ?? null,
    });
    return { id, done };
  }

  retry(id: string): void {
    this.apply({ type: "retry", nowMs: this.now(), id });
  }

  discard(id: string): void {
    this.apply({ type: "discard", nowMs: this.now(), id });
    this.waiters.get(id)?.reject(new Error("Deleted before it was sent."));
    this.waiters.delete(id);
  }

  /** The reachability the app saw: a poll that failed on the connection, or any call that worked. */
  setOnline(online: boolean): void {
    if (this.state && this.state.online === online) return;
    this.apply({ type: "online", nowMs: this.now(), online });
  }

  /** The app came to the foreground or a timer fired: retry what is due. */
  tick(): void {
    this.apply({ type: "tick", nowMs: this.now() });
  }

  /** Sign-out: forget everything, on disk too. */
  async clear(): Promise<void> {
    this.state = outboxInitial();
    for (const w of this.waiters.values()) w.reject(new Error("Signed out."));
    this.waiters.clear();
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
    this.bump();
    await this.deps.store.remove(SLOT);
  }

  /* ---- the state machine ---- */

  private async load(): Promise<void> {
    if (!this.state) return;
    try {
      const raw = await this.deps.store.get(SLOT);
      if (raw) {
        const saved = JSON.parse(raw) as OutboxState;
        if (saved && saved.schema === this.state.schema && Array.isArray(saved.messages)) this.state = { ...saved, online: true, lost: 0 };
      }
    } catch {
      /* an unreadable saved outbox is dropped; nothing is posted from a guess */
    }
    // Whatever was being posted when the app quit is looked for in its thread, never posted blindly.
    this.apply({ type: "resume", nowMs: this.now() });
    // A new launch is a fresh chance: what waited out a lost connection last time (and its backoff) goes now.
    this.apply({ type: "online", nowMs: this.now(), online: true });
  }

  private apply(action: OutboxAction): void {
    if (!this.state) return;
    const r = outboxReduce(this.state, action);
    if (!r) return;
    this.state = r.state;
    this.persist();
    this.bump();
    for (const e of r.effects) void this.run(e);
    this.settleFailed();
  }

  private persist(): void {
    const snapshot = JSON.stringify(this.state);
    this.persisting = this.persisting.then(() => this.deps.store.set(SLOT, snapshot)).catch(() => {});
  }

  private bump(): void {
    this.version++;
    for (const l of this.listeners) l();
  }

  /** A message that gave up rejects whoever waits on it. */
  private settleFailed(): void {
    if (!this.state) return;
    for (const m of this.state.messages) {
      if (m.state === "failed" && this.waiters.has(m.id)) {
        this.waiters.get(m.id)!.reject(new Error(m.error ?? "The computer did not take it."));
        this.waiters.delete(m.id);
      }
    }
  }

  private async run(e: OutboxEffect): Promise<void> {
    if (e.type === "wakeAt") return this.wake(e.atMs);
    if (e.type === "verifyDelivery") return this.verify(e.id, e.space);
    return this.deliver(e.id);
  }

  private wake(atMs: number): void {
    if (this.timer && this.timerAt <= atMs) return;
    if (this.timer) clearTimeout(this.timer);
    this.timerAt = atMs;
    this.timer = setTimeout(
      () => {
        this.timer = null;
        this.tick();
      },
      Math.max(0, atMs - this.now()),
    );
  }

  private async post(
    backend: Backend,
    m: { space: string; kind: string; text: string; replyTo: number | null; mode: string; extra: Record<string, unknown> | null; id: string },
  ): Promise<Delivered> {
    const extra = m.extra ?? {};
    if (m.kind === "task") {
      if (!backend.startTask) throw new Error(backend.capabilities.why.startTask ?? "This account cannot start a task from the phone yet.");
      const task = await backend.startTask(extra.request as never);
      return { id: m.id, space: task.space, kind: m.kind, entry: null, task };
    }
    if (m.kind === "answer") {
      const entry = await backend.answer(m.space, extra.approval as number, extra.approve === true, {
        scope: (extra.scope as "once" | "space" | undefined) ?? undefined,
        clientId: m.id,
      });
      return { id: m.id, space: m.space, kind: m.kind, entry, task: null };
    }
    const entry = await backend.post(m.space, m.text, {
      replyTo: m.replyTo,
      view: (extra.view as never) ?? null,
      clientId: m.id,
      mode: (m.mode as "send" | "queue" | "steer") ?? "send",
      agent: (extra.agent as string | undefined) ?? null,
    });
    return { id: m.id, space: m.space, kind: m.kind, entry, task: null };
  }

  private async deliver(id: string): Promise<void> {
    const m = this.message(id);
    if (!m) return;
    const backend = this.deps.backend();
    if (!backend) return this.apply({ type: "deliveryFailed", nowMs: this.now(), id, network: true, error: "" });
    try {
      const d = await this.post(backend, m);
      this.results.set(id, d);
      this.apply({ type: "delivered", nowMs: this.now(), id });
      for (const l of this.deliveredListeners) l(d);
      this.waiters.get(id)?.resolve(d);
      this.waiters.delete(id);
    } catch (e) {
      const network = isNetworkError(e);
      this.apply({ type: "deliveryFailed", nowMs: this.now(), id, network, error: errorText(e) });
      // A delivery that fails on the connection is the app's first news that it is offline.
      if (network) this.setOnline(false);
    }
  }

  /** Without a state machine: one try, the failure is the caller's. */
  private async deliverDirect(id: string, input: SendInput & { kind: string }): Promise<void> {
    const backend = this.deps.backend();
    try {
      if (!backend) throw new Error("Not connected.");
      const d = await this.post(backend, {
        id,
        space: input.space,
        kind: input.kind,
        text: input.text ?? "",
        replyTo: input.replyTo ?? null,
        mode: input.mode ?? "send",
        extra: input.extra ?? null,
      });
      for (const l of this.deliveredListeners) l(d);
      this.waiters.get(id)?.resolve(d);
    } catch (e) {
      this.waiters.get(id)?.reject(e instanceof Error ? e : new Error(errorText(e)));
    }
    this.waiters.delete(id);
  }

  /** A message that was being posted when the app quit: is it already in the thread? */
  private async verify(id: string, space: string): Promise<void> {
    if (this.verifying.has(id)) return;
    const m = this.message(id);
    if (!m) return;
    // A task has no thread yet to look in; the daemon takes the same client id as the same task, so it goes again.
    if (m.kind === "task") return this.apply({ type: "threadHas", nowMs: this.now(), id, found: false });
    const backend = this.deps.backend();
    this.verifying.add(id);
    try {
      if (!backend) throw new Error("Not connected.");
      const page = await backend.readThread(space, { limit: 80 });
      const lite: EntryLite[] = page.entries.map((e) => ({
        id: e.id,
        clientId: e.clientId ?? e.answer?.clientId ?? null,
        authorId: e.author.id,
        text: e.text,
        tsMs: e.tsMs,
      }));
      const found = outboxFound(m, lite, this.deps.me() ?? page.me ?? "") ?? false;
      this.verifying.delete(id);
      this.apply({ type: "threadHas", nowMs: this.now(), id, found });
    } catch {
      this.verifying.delete(id);
      // Cannot tell yet: look again shortly. The message stays claimed, so nothing is posted twice meanwhile.
      setTimeout(() => void this.verify(id, space), 3_000);
    }
  }
}

/** Whether the outbox can hold messages at all here (the app core is linked). */
export const outboxAvailable = (): boolean => core.available && outboxInitial() !== null;
