// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/** The folded thread plus what lives beside it (tool calls, the live agents, unsent messages), as the rows the list draws. */

import { core } from "../../core";
import type { OutboxRow } from "../../core/phone";
import type { ThreadItem } from "../../data/thread/fold";
import type { ToolCall, ToolGroup } from "../../data/thread/phone-protocol";
import type { LiveAgent, ThreadEntry } from "../../data/types";
import type { ChildNote, LiveChild } from "../../data/thread/protocol";
import { dayLine } from "../../ui/time";

export type { ToolCall };

export type Row =
  | { type: "day"; key: string; text: string }
  | { type: "message"; key: string; entry: ThreadEntry; mine: boolean; showAuthor: boolean; quote: { author: string; text: string } | null }
  | { type: "approval"; key: string; entry: ThreadEntry; answered: Answered | null }
  | { type: "system"; key: string; text: string }
  | { type: "unread"; key: string }
  | { type: "tools"; key: string; calls: ToolCall[] }
  | { type: "live"; key: string; agent: string; stage: string; text: string; children: LiveChild[] }
  | { type: "subagent"; key: string; sub: Extract<ChildNote, { kind: "subagent" }> }
  | { type: "creator"; key: string; entry: ThreadEntry }
  | { type: "child_space"; key: string; space: Extract<ChildNote, { kind: "child_space" }> }
  | { type: "pending"; key: string; row: OutboxRow };

export interface Answered {
  approve: boolean;
  by: string;
  tsMs: number;
  scope: "once" | "space" | null;
}

export interface AuthorView {
  name: string;
  kind: "person" | "agent" | "system";
  /** The small tag beside the name (`APP` on Cua's own entries). */
  tag: string | null;
  /** Cua's own entries use the Cua Spaces app icon. */
  appIcon: boolean;
}

/**
 * How an author shows: the app core's `threadCards.author` (Cua's own entries are the `Cua` APP with the app icon).
 * Where the core is not linked (Android until it is built there) the plain name, with no tag: the rule is not
 * repeated here.
 */
export function authorView(a: { kind: string; id: string; name: string }): AuthorView {
  const v = core.tryCall<{ kind: string; name: string; tag: string | null; avatar: string | null }>("threadCards.author", { author: a });
  if (v) return { name: v.name, kind: v.kind as AuthorView["kind"], tag: v.tag, appIcon: v.avatar === "cua-app-icon" };
  return { name: a.name, kind: a.kind as AuthorView["kind"], tag: null, appIcon: false };
}

const GAP_MS = 30 * 60_000;
const GROUP_MS = 5 * 60_000;

export function buildRows(opts: {
  items: ThreadItem[];
  me: string | null;
  live: LiveAgent[];
  pending: OutboxRow[];
  tools: ToolGroup[];
  now: number;
  agentName(handle: string): string;
}): Row[] {
  const rows: Row[] = [];
  let lastTs = 0;
  let lastAuthor = "";
  let lastWasMessage = false;
  // Calls of every agent that followed the same message are one group, in the order they happened.
  const toolsAfter = new Map<number, ToolCall[]>();
  for (const t of opts.tools)
    toolsAfter.set(
      t.afterSeq,
      [...(toolsAfter.get(t.afterSeq) ?? []), ...t.calls].sort((a, b) => a.atMs - b.atMs),
    );
  for (const it of opts.items) {
    if (it.type === "unread") {
      rows.push({ type: "unread", key: it.key });
      lastWasMessage = false;
      continue;
    }
    const e = it.entry;
    if (e.tsMs - lastTs > GAP_MS || rows.length === 0) {
      rows.push({ type: "day", key: `d${e.seq}`, text: dayLine(e.tsMs, opts.now) });
      lastWasMessage = false;
    }
    lastTs = e.tsMs;
    if (it.type === "system") {
      rows.push({ type: "system", key: it.key, text: it.text });
      lastWasMessage = false;
      continue;
    }
    if (it.type === "subagent") {
      rows.push({ type: "subagent", key: it.key, sub: it.sub });
      lastWasMessage = false;
      continue;
    }
    if (it.type === "child_space") {
      rows.push({ type: "child_space", key: it.key, space: it.space });
      lastWasMessage = false;
      continue;
    }
    if (it.type === "approval" || it.type === "join_request") {
      rows.push({
        type: "approval",
        key: it.key,
        entry: e,
        answered: it.answer ? { approve: it.answer.approve, by: it.answer.by.name, tsMs: it.answer.tsMs, scope: it.answer.scope } : null,
      });
      lastWasMessage = false;
      lastAuthor = "";
      continue;
    }
    // A created Space's creator: its header alone (the row follows).
    if (it.type === "message" && it.entry.id.endsWith(":header")) {
      rows.push({ type: "creator", key: it.key, entry: e });
      lastWasMessage = false;
      lastAuthor = "";
      continue;
    }
    const mine = !!opts.me && e.author.id === opts.me;
    const showAuthor = !mine && (!lastWasMessage || lastAuthor !== e.author.id || e.tsMs - lastTs > GROUP_MS);
    // A reply names what it answers only when that message is not right above it.
    const prevSeq = rows.length && rows[rows.length - 1]!.type === "message" ? (rows[rows.length - 1] as { entry: ThreadEntry }).entry.seq : null;
    const quote = it.type === "message" && it.quote && it.quote.seq !== prevSeq ? { author: it.quote.author.name, text: it.quote.text } : null;
    rows.push({ type: "message", key: it.key, entry: e, mine, showAuthor, quote });
    lastWasMessage = true;
    lastAuthor = e.author.id;
    const calls = toolsAfter.get(e.seq);
    if (calls) rows.push({ type: "tools", key: `tools${e.seq}`, calls });
  }
  for (const l of opts.live)
    rows.push({
      type: "live",
      key: `live-${l.agent}`,
      agent: opts.agentName(l.agent.replace(/^@/, "")),
      stage: l.stage,
      text: l.text ?? "",
      children: l.children ?? [],
    });
  // Messages the machine has not taken yet (waiting, sending or failed) close the thread, as the person's own words; an answer to an
  // approval shows on its card instead.
  for (const p of opts.pending) if (p.kind === "post") rows.push({ type: "pending", key: `pending-${p.id}`, row: p });
  return rows;
}
