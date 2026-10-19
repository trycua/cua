// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * One presence session per Space per page, shared by every stream surface on
 * the page (the desktop viewer, or each window panel of a multi-window
 * viewer), so one person is one participant however many panels they have
 * open. Reference-counted: the last release leaves.
 */

import { PresenceView, HEARTBEAT_INTERVAL_MS } from "@trycua/cua/spaces/presence";

import type { PresenceBridge, PresenceJoinInfo, PresencePointer } from "../../native/presence";

/** How often staleness is checked (a timer, not rAF, so hidden pages still expire). */
export const EXPIRE_EVERY_MS = 1_000;

export class SharedPresence {
  readonly view: PresenceView;
  private readonly listeners = new Set<() => void>();
  private unsubscribe: (() => void) | undefined;
  private timer: ReturnType<typeof setInterval> | undefined;

  constructor(
    readonly bridge: PresenceBridge,
    readonly info: PresenceJoinInfo,
    readonly now: () => number,
  ) {
    this.view = PresenceView.from(info.me, info.members, info.delayMs, now());
    this.view.setHeartbeatIntervalMs(HEARTBEAT_INTERVAL_MS);
  }

  async start(): Promise<void> {
    this.unsubscribe = await this.bridge.onEvent(this.info.handle, (event) => {
      if (this.view.apply(event, this.now())) this.notify();
    });
    this.timer = setInterval(() => {
      if (this.view.expire(this.now()).length > 0) this.notify();
    }, EXPIRE_EVERY_MS);
  }

  get me(): string {
    return this.info.me.participantId;
  }

  /** My color, as the server assigned it. */
  get myColor(): string {
    return this.view.participant(this.me)?.color || this.info.me.color;
  }

  subscribe(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  private notify(): void {
    for (const l of this.listeners) l();
  }

  publish(pointer: PresencePointer): void {
    void this.bridge.publish(this.info.handle, pointer).catch(() => {});
  }

  stop(): void {
    this.unsubscribe?.();
    if (this.timer) clearInterval(this.timer);
    this.listeners.clear();
    void this.bridge.leave(this.info.handle).catch(() => {});
  }
}

interface Entry {
  promise: Promise<SharedPresence | null>;
  refs: number;
}

const sessions = new Map<string, Entry>();

/**
 * Joins (or reuses) the page's presence session for `spaceId`. Resolves to
 * null when there is no bridge or the Space has no presence. Call the
 * returned release exactly once.
 */
export function acquirePresence(
  bridge: PresenceBridge | null,
  spaceId: string,
  now: () => number = Date.now,
): { session: Promise<SharedPresence | null>; release: () => void } {
  if (!bridge) return { session: Promise.resolve(null), release: () => {} };
  let entry = sessions.get(spaceId);
  if (!entry) {
    const promise = bridge
      .join(spaceId)
      .then(async (info) => {
        const s = new SharedPresence(bridge, info, now);
        await s.start();
        return s;
      })
      .catch(() => null);
    entry = { promise, refs: 0 };
    sessions.set(spaceId, entry);
  }
  entry.refs++;
  const held = entry;
  let released = false;
  return {
    session: held.promise,
    release: () => {
      if (released) return;
      released = true;
      held.refs--;
      if (held.refs > 0) return;
      if (sessions.get(spaceId) === held) sessions.delete(spaceId);
      void held.promise.then((s) => s?.stop());
    },
  };
}
