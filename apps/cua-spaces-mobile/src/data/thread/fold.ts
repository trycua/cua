// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The Space thread folded into what the screen draws (`foldThread`), as the
 * web app's `bridge/thread.ts` does (CUA-2640): one row per entry, in order;
 * a task's state from its `task_update`s; an approval open until an `answer`
 * names it; one muted note per `system` entry; the "since you were here"
 * divider before the first entry after the reader's cursor.
 */

import type { ChildNote, ThreadAuthor, ThreadEntry, TaskState } from "./protocol";

export interface ThreadAnswer {
  approve: boolean;
  by: ThreadAuthor;
  tsMs: number;
  note: string | null;
  /** What was allowed: this approval only, or this kind of action for the Space. */
  scope: "once" | "space" | null;
}

/** The message a reply answers (the page quotes it when it is not right above). */
export interface ThreadQuote {
  seq: number;
  author: ThreadAuthor;
  text: string;
}

export type ThreadItem =
  | {
      type: "message";
      key: string;
      entry: ThreadEntry;
      /** The message it replies to, when it replies to one. */
      quote: ThreadQuote | null;
      /** What the task it answers asked, for Retry (a failed reply sends it again). */
      taskText: string | null;
    }
  | {
      type: "task";
      key: string;
      entry: ThreadEntry;
      state: TaskState;
      session: number | null;
    }
  | { type: "approval"; key: string; entry: ThreadEntry; answer: ThreadAnswer | null }
  /** Someone's agent asks to join (CUA-2264): answered like an approval. */
  | { type: "join_request"; key: string; entry: ThreadEntry; answer: ThreadAnswer | null }
  | { type: "system"; key: string; entry: ThreadEntry; text: string }
  /** A subagent the agent started: one row for all its notes (running, then done or failed), at the first. */
  | { type: "subagent"; key: string; entry: ThreadEntry; sub: Extract<ChildNote, { kind: "subagent" }> }
  /** A child Space the agent made. */
  | { type: "child_space"; key: string; entry: ThreadEntry; space: Extract<ChildNote, { kind: "child_space" }> }
  | { type: "unread"; key: string };

/** The thread's entries (in any order, duplicates allowed) as the page draws them: one row each, in sequence. */
export function foldThread(entries: readonly ThreadEntry[], opts: { readSeq?: number | null; me?: string | null } = {}): ThreadItem[] {
  const sorted = dedupe(entries);
  const items: ThreadItem[] = [];
  const bySeq = new Map<number, ThreadEntry>(sorted.map((e) => [e.seq, e]));
  const tasks = new Map<number, Extract<ThreadItem, { type: "task" }>>();
  const approvals = new Map<number, Extract<ThreadItem, { type: "approval" | "join_request" }>>();
  const subs = new Map<string, Extract<ThreadItem, { type: "subagent" }>>();
  const kids = new Map<string, Extract<ThreadItem, { type: "child_space" }>>();
  const answered = new Set<string>();
  for (const e of sorted) {
    // The copy of a message that tasks one more agent: the original is the row.
    if (e.fanoutOf != null) continue;
    if (e.kind === "task_update") {
      const u = e.update;
      const task = u ? tasks.get(u.task) : undefined;
      if (u && task) {
        task.state = u.state;
        task.session = u.session ?? task.session;
      }
      if (u?.note) items.push({ type: "system", key: `s${e.seq}`, entry: e, text: u.note });
      continue;
    }
    if (e.kind === "answer" && e.answer) {
      const a = approvals.get(e.answer.approval);
      if (a) {
        a.answer = { approve: e.answer.approve, by: e.author, tsMs: e.tsMs, note: e.answer.note, scope: e.answer.scope ?? null };
        continue;
      }
      items.push({ type: "system", key: `s${e.seq}`, entry: e, text: `${e.author.name} ${e.answer.approve ? "approved" : "denied"} a request` });
      continue;
    }
    if (e.kind === "system" && e.child?.kind === "subagent") {
      const known = subs.get(e.child.id);
      if (known) {
        known.sub = e.child;
      } else {
        const header = creatorHeader(items, e, e.child.agent, null);
        if (header) items.push(header);
        const item = { type: "subagent" as const, key: `u${e.seq}`, entry: e, sub: e.child };
        subs.set(e.child.id, item);
        items.push(item);
      }
      continue;
    }
    if (e.kind === "system" && e.child?.kind === "child_space") {
      // The notes of one create (its steps, then ready or failed) are one row, in the order it started.
      const known = e.child.key ? kids.get(e.child.key) : undefined;
      if (known) {
        known.space = e.child;
      } else {
        // The row sits under its creator's turn, as a subagent's does (its header comes with the row when the creator has not spoken just above).
        const header = creatorHeader(items, e, e.child.agent, e.child.by ?? null);
        if (header) items.push(header);
        const item = { type: "child_space" as const, key: `c${e.seq}`, entry: e, space: e.child };
        if (e.child.key) kids.set(e.child.key, item);
        items.push(item);
      }
      continue;
    }
    if (e.kind === "system") {
      items.push({ type: "system", key: `s${e.seq}`, entry: e, text: e.text });
      continue;
    }
    if (e.kind === "join_request" && e.joinRequest) {
      const item = { type: "join_request" as const, key: `j${e.seq}`, entry: e, answer: null };
      approvals.set(e.seq, item);
      items.push(item);
      continue;
    }
    if (e.kind === "approval" && e.approval) {
      const item = { type: "approval" as const, key: `a${e.seq}`, entry: e, answer: null };
      approvals.set(e.seq, item);
      items.push(item);
      continue;
    }
    if (e.task && e.replyTo === null) {
      const item = { type: "task" as const, key: `t${e.seq}`, entry: e, state: "queued" as TaskState, session: e.task.session };
      tasks.set(e.seq, item);
      items.push(item);
      continue;
    }
    const parent = e.replyTo !== null ? bySeq.get(e.replyTo) : undefined;
    // An agent that says several things under one message names it once: the later ones follow the first.
    const said = parent ? `${parent.seq}:${e.author.id}` : null;
    const again = said !== null && answered.has(said);
    if (said !== null) answered.add(said);
    items.push({
      type: "message",
      key: `m${e.seq}`,
      entry: e,
      quote: parent && !again ? { seq: parent.seq, author: parent.author, text: parent.text } : null,
      taskText: parent?.task ? parent.text : null,
    });
  }
  const readSeq = opts.readSeq;
  if (readSeq == null) return items;
  const first = items.findIndex(
    (i) =>
      i.type !== "unread" &&
      i.type !== "system" &&
      i.type !== "subagent" &&
      i.type !== "child_space" &&
      !isCreatorHeader(i) &&
      unreadIn(i, readSeq, opts.me ?? null),
  );
  if (first < 0) return items;
  // A note just above the first unread entry belongs below the divider.
  let at = first;
  while (at > 0 && items[at - 1]!.type === "system" && (items[at - 1] as { entry: ThreadEntry }).entry.seq > readSeq) at--;
  items.splice(at, 0, { type: "unread", key: "unread" });
  return items;
}

function unreadIn(item: Exclude<ThreadItem, { type: "unread" }>, readSeq: number, me: string | null): boolean {
  return item.entry.seq > readSeq && item.entry.author.id !== me;
}

function dedupe(entries: readonly ThreadEntry[]): ThreadEntry[] {
  const bySeq = new Map<number, ThreadEntry>();
  for (const e of entries) bySeq.set(e.seq, e);
  return [...bySeq.values()].sort((a, b) => a.seq - b.seq);
}

/** The reader's cursor when the daemon does not say it: the entries
 * `unread` counts are the last ones someone else wrote. */
export function readSeqFrom(entries: readonly ThreadEntry[], unread: number, me: string | null): number | null {
  if (unread <= 0) return entries.length ? entries[entries.length - 1]!.seq : 0;
  const others = entries.filter((e) => e.author.id !== me && e.kind !== "task_update");
  if (unread > others.length) return null;
  return others[others.length - unread]!.seq - 1;
}

/** The textless message that heads a created Space's row (never unread, never a quote target). */
export const isCreatorHeader = (i: ThreadItem): boolean => i.type === "message" && i.entry.id.endsWith(":header");

/**
 * The header a created Space's row needs above it, or null when its creator's own message or earlier rows already sit
 * right above. The creator is the agent that made it, or the person (`by`) when a person did; with neither the row stands alone.
 */
function creatorHeader(items: readonly ThreadItem[], note: ThreadEntry, agent: string, by: { id: string; name: string } | null): ThreadItem | null {
  const author: ThreadEntry["author"] | null = by
    ? { kind: "person", id: by.id, name: by.name }
    : agent
      ? { kind: "agent", id: `agent:${agent}`, name: agent }
      : null;
  if (!author) return null;
  for (let i = items.length - 1; i >= 0; i--) {
    const it = items[i]!;
    if (it.type === "unread" || it.type === "system") continue;
    const same = (a: ThreadEntry["author"]) => a.kind === author.kind && (a.id === author.id || (a.kind === "agent" && a.name === author.name));
    if (it.type === "message" && same(it.entry.author)) return null;
    if (it.type === "child_space" || it.type === "subagent") {
      const c = it.type === "child_space" ? it.space : it.sub;
      const mine = c.kind === "child_space" ? (c.by ? !!by && c.by.id === by.id : !by && !!agent && c.agent === agent) : !by && c.agent === agent;
      if (mine) return null;
    }
    break;
  }
  return {
    type: "message",
    key: `h${note.seq}`,
    entry: { ...note, seq: note.seq - 0.5, id: `${note.id}:header`, author, kind: "message", text: "", child: null, noteFor: null, event: null },
    quote: null,
    taskText: null,
  };
}
