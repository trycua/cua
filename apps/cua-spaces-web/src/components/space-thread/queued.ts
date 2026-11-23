// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Messages written to a Space that is still being made: Ask first's job
 * (`@claude <job>`) and anything typed in its composer meanwhile. They show
 * in the thread as ordinary messages that "send when the Space is ready";
 * once the Space exists they are sent in order, through the same summon as
 * any message, and each leaves the queue as it is posted.
 */

import { useMemo } from "react";
import { create } from "zustand";

import type { SpaceThread, ThreadEntry, ThreadItem } from "@/bridge";
import { usePreparedQueue } from "@/lib/prepare/store";

interface Queued {
  id: string;
  text: string;
  tsMs: number;
}

const useQueue = create<{ byPending: Record<string, Queued[]>; resolved: Record<string, string> }>(() => ({ byPending: {}, resolved: {} }));

let n = 0;

/**
 * Where messages for the home being prepared go instead of the in-memory
 * queue: the preparing driver keeps them (so a quit loses none) and sends
 * each exactly once. Set by `PrepareRoot`.
 */
export interface PrepareSink {
  /** The row (or Space) is the home being prepared. */
  owns(spaceId: string): boolean;
  send(text: string): void;
}
let prepareSink: PrepareSink | null = null;
export function setPrepareSink(sink: PrepareSink | null): void {
  prepareSink = sink;
}

/** Queues `text` for the Space being made under `pendingId`. */
export function enqueue(pendingId: string, text: string): void {
  const q: Queued = { id: `q${++n}`, text: text.trim(), tsMs: Date.now() };
  if (!q.text) return;
  useQueue.setState((s) => ({ byPending: { ...s.byPending, [pendingId]: [...(s.byPending[pendingId] ?? []), q] } }));
}

/** The create finished: `pendingId` is `spaceId` now (its queue shows there until sent). */
export function resolveQueue(pendingId: string, spaceId: string): void {
  useQueue.setState((s) => ({ resolved: { ...s.resolved, [pendingId]: spaceId } }));
}

/**
 * Drops what `pendingId` shows without sending it: the message was posted by another path (a home's briefed
 * first message), so it is in the thread for real now.
 */
export function dropQueue(pendingId: string): void {
  useQueue.setState((s) => {
    const { [pendingId]: _gone, ...rest } = s.byPending;
    return { byPending: rest };
  });
}

/** Sends the queue of `pendingId` in order; each message leaves it once posted. Safe to call twice. */
export async function flushQueue(pendingId: string, send: (text: string) => Promise<void>): Promise<void> {
  for (;;) {
    const next = useQueue.getState().byPending[pendingId]?.[0];
    if (!next) return;
    // Taken before sending, so a second flusher never sends it again.
    useQueue.setState((s) => ({ byPending: { ...s.byPending, [pendingId]: (s.byPending[pendingId] ?? []).filter((q) => q.id !== next.id) } }));
    await send(next.text);
  }
}

/** Tests start clean. */
export function resetQueued(): void {
  useQueue.setState({ byPending: {}, resolved: {} });
}

/** A queued message as a thread message from you (negative seqs: never a real entry's). */
function entryOf(q: Queued, i: number, me: string, name = ""): ThreadEntry {
  return {
    seq: -1 - i,
    id: q.id,
    tsMs: q.tsMs,
    author: { kind: "person", id: me, name },
    kind: "message",
    text: q.text,
    mentions: [],
    replyTo: null,
    task: null,
    attachments: [],
    approval: null,
    answer: null,
    noteFor: null,
    update: null,
    runsOnKeysOf: null,
    event: null,
  } as ThreadEntry;
}

/** The queued messages that belong to `spaceId` (its own pending id, or a pending id that became it). */
export function useQueued(spaceId: string): Queued[] {
  const { byPending, resolved } = useQueue();
  const prepared = usePreparedQueue(spaceId);
  return useMemo(
    () => [
      ...Object.entries(byPending).flatMap(([pending, qs]) => (pending === spaceId || resolved[pending] === spaceId ? qs : [])),
      // The home's messages kept by the preparing driver, until each is in the thread.
      ...prepared.map((m): Queued => ({ id: m.id, text: m.text, tsMs: m.sentMs })),
    ],
    [byPending, resolved, spaceId, prepared],
  );
}

/** Queued messages as thread items. */
export const queuedItems = (qs: readonly Queued[], me: string, name: string): ThreadItem[] =>
  qs.map((q, i) => ({ type: "message" as const, key: `queued:${q.id}`, entry: entryOf(q, i, me, name), quote: null, taskText: null }));

/**
 * The thread of a Space still being made: empty (its queued messages are
 * drawn by the page, `useQueued`), and a composer that queues more. No
 * members, shares or agents until it exists.
 */
export function creatingThread(pendingId: string): SpaceThread {
  return {
    status: "ready",
    items: [],
    entries: [],
    members: [],
    shares: [],
    live: [],
    me: "me",
    role: null,
    error: null,
    hasMoreBefore: false,
    loadEarlier: async () => {},
    post: async (text) => {
      if (prepareSink?.owns(pendingId)) prepareSink.send(text.trim());
      else enqueue(pendingId, text);
      return null;
    },
    answer: async () => {},
    setRights: async () => {},
    removeAgent: null,
  };
}
