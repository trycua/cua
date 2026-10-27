// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The Agents tab as a chat (CUA-2266). An agent is a member of one or more
 * Spaces' threads; its DM with you is a view of those threads
 * (`thread.read {with}`), and sending is an @mention task in one of them.
 *
 * - `useAgentChats()` lists the agents across every Space
 *   (`thread.members {overview}` per Space). One agent in several Spaces
 *   is one row, keyed on who owns it, its harness and its name (`chatKey`),
 *   since handles differ per daemon. Each row has its status line, unread
 *   count and what waits for you.
 * - `useAgentDm(chat, space)` reads the DM in each of its Spaces (or one),
 *   follows it, marks it read while visible, posts @mention tasks and
 *   answers approvals.
 */

import { useCallback, useEffect, useState } from "react";

import { useBridge, useSpaces } from "./hooks";
import type { Space } from "./contracts/spaces";
import { foldThread, readSeqFrom, type ThreadItem } from "./thread";
import { isMissingThreadTool, THREAD_FOLLOW_MAX_MS, type AgentMember, type MemberIdentity, type NeedsYou, type ThreadEntry, type ThreadRole } from "./ops/thread";

/** One agent's membership in one Space. */
export interface AgentChatSpace {
  spaceId: string;
  spaceName: string;
  os: Space["os"];
  /** Its handle there (`@codex`). */
  handle: string;
  member: AgentMember;
  /** Your role in that Space (null: a daemon that doesn't say, the owner's). */
  role: ThreadRole | null;
}

/** Something that waits for you: an approval you may answer, or the agent's question to you. */
export interface ChatNeed extends NeedsYou {
  spaceId: string;
  spaceName: string;
}

export type ChatTone = "you" | "working" | "idle" | "asleep" | "failed";

/** One agent, across its Spaces. */
export interface AgentChat {
  key: string;
  /** The name it chose. */
  label: string;
  identity: MemberIdentity | null;
  harness: string;
  spaces: AgentChatSpace[];
  /** Its DM entries you haven't read, in all its Spaces. */
  unread: number;
  needsYou: ChatNeed[];
  tone: ChatTone;
  /** "Waiting for you in release-checks", "Bumping the retry timeout in ubuntu-build", "Asleep". */
  line: string;
  /** The last thing said in its DM, for ordering and the age. */
  lastMs: number;
}

/** One agent across Spaces: the same owner, harness and name. Without an identity, its handle in its Space. */
export function chatKey(spaceId: string, m: AgentMember): string {
  const i = m.identity;
  if (!i) return `${spaceId}/${m.name}`;
  // The same rule as `identityKey` on the thread's card (its Activity link opens this row).
  return `${i.owner.id}/${i.harness.id || "other"}/${i.displayName.trim().toLowerCase()}`;
}

const WORKING = new Set(["working", "claimed"]);

/** Merges each Space's members (with `overview`) into one row per agent. */
export function mergeChats(perSpace: readonly { space: Space; role: ThreadRole | null; members: readonly AgentMember[] }[]): AgentChat[] {
  const by = new Map<string, AgentChat>();
  for (const { space, role, members } of perSpace) {
    for (const m of members) {
      if (m.identity?.state === "pending") continue;
      const key = chatKey(space.id, m);
      let c = by.get(key);
      if (!c) {
        c = { key, label: m.identity?.displayName || m.name, identity: m.identity, harness: m.identity?.harness.id || m.harness, spaces: [], unread: 0, needsYou: [], tone: "idle", line: "", lastMs: 0 };
        by.set(key, c);
      }
      c.spaces.push({ spaceId: space.id, spaceName: space.name, os: space.os, handle: m.name, member: m, role });
      c.unread += m.dm?.unread ?? 0;
      c.lastMs = Math.max(c.lastMs, m.dm?.last?.tsMs ?? 0);
      for (const n of m.needsYou ?? []) c.needsYou.push({ ...n, spaceId: space.id, spaceName: space.name });
    }
  }
  const out = [...by.values()];
  for (const c of out) {
    c.needsYou.sort((a, b) => b.tsMs - a.tsMs);
    c.lastMs = Math.max(c.lastMs, ...c.needsYou.map((n) => n.tsMs));
    const working = c.spaces.find((s) => WORKING.has(s.member.status));
    const failed = c.spaces.find((s) => s.member.status === "failed");
    if (c.needsYou.length) {
      c.tone = "you";
      c.line = `Waiting for you in ${c.needsYou[0]!.spaceName}`;
    } else if (working) {
      c.tone = "working";
      c.line = `${sentence(working.member.activity) ?? "Working"} in ${working.spaceName}`;
    } else if (failed) {
      c.tone = "failed";
      c.line = `Failed in ${failed.spaceName}`;
    } else if (c.spaces.every((s) => s.member.status === "asleep" || s.member.status === "paused")) {
      c.tone = "asleep";
      c.line = c.spaces.length > 1 ? `Asleep in ${c.spaces.length} Spaces` : `Asleep in ${c.spaces[0]!.spaceName}`;
    } else {
      c.tone = "idle";
      c.line = c.spaces.length > 1 ? `In ${c.spaces.length} Spaces` : `In ${c.spaces[0]!.spaceName}`;
    }
  }
  // Waiting for you, then working, then the most recent.
  const rank = (c: AgentChat) => (c.tone === "you" ? 0 : c.tone === "working" ? 1 : 2);
  return out.sort((a, b) => rank(a) - rank(b) || b.lastMs - a.lastMs || a.label.localeCompare(b.label));
}

/** "bumping the retry timeout" reads "Bumping the retry timeout"; a line that names its own Space keeps only the action. */
function sentence(s: string | null): string | null {
  if (!s) return null;
  const t = s.replace(/\s+in\s+\S+$/i, "").replace(/^waiting for you$/i, "Waiting");
  return t.charAt(0).toUpperCase() + t.slice(1);
}

export const CHATS_POLL_MS = 5_000;

export interface AgentChats {
  /** null while the first read is under way. */
  chats: AgentChat[] | null;
  /** Spaces with a thread (where an agent can be added). */
  spaces: Space[];
  /** Your author id, as the threads say it. */
  me: string | null;
  /** Read again now (after adding or removing an agent). */
  refresh(): void;
}

/** Every agent in every Space you can see, as the Agents tab's list. */
export function useAgentChats(): AgentChats {
  const { data } = useBridge();
  const { data: spaces } = useSpaces();
  const [chats, setChats] = useState<AgentChat[] | null>(null);
  const [withThread, setWithThread] = useState<Space[]>([]);
  const [me, setMe] = useState<string | null>(null);
  const [round, setRound] = useState(0);
  const list = (spaces ?? []).filter((s) => !s.id.startsWith("pending:"));
  const ids = list.map((s) => s.id).join("\n");
  useEffect(() => {
    if (!data || spaces === undefined) return;
    let alive = true;
    const read = async () => {
      const got = await Promise.all(
        list.map(async (space) => {
          try {
            const [{ members }, page] = await Promise.all([data.call("thread.members", { spaceId: space.id, overview: true }), data.call("thread.read", { spaceId: space.id, limit: 1 })]);
            return { space, role: page.role, members, me: page.me };
          } catch (e) {
            // No thread there (an old daemon, a stopped machine): not in the list.
            if (!isMissingThreadTool(e)) console.debug("agents: couldn't read", space.id, e);
            return null;
          }
        }),
      );
      if (!alive) return;
      const ok = got.filter((g): g is NonNullable<typeof g> => g !== null);
      setWithThread(ok.map((g) => g.space));
      setMe(ok.find((g) => g.me)?.me ?? null);
      setChats(mergeChats(ok));
    };
    void read();
    const t = setInterval(() => void read(), CHATS_POLL_MS);
    return () => {
      alive = false;
      clearInterval(t);
    };
    // `list` follows `ids`.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [data, ids, round, spaces === undefined]);
  return { chats, spaces: withThread, me, refresh: useCallback(() => setRound((r) => r + 1), []) };
}

/** One DM item, with the Space it came from. */
export interface DmItem {
  item: ThreadItem;
  spaceId: string;
  /** For ordering across Spaces. */
  tsMs: number;
}

export interface AgentDm {
  status: "loading" | "ready" | "error";
  error: string | null;
  /** Oldest first, across the Spaces shown. */
  items: DmItem[];
  /** Each Space's DM entries (a roster per Space reads its authors). */
  entries: Record<string, ThreadEntry[]>;
  /** Your author id, when known. */
  me: string | null;
  /** Sends `text` to the agent in `spaceId`: an @mention task in that Space's thread. */
  send(spaceId: string, text: string): Promise<void>;
  answer(spaceId: string, approval: number, approve: boolean): Promise<void>;
}

const visible = () => typeof document === "undefined" || document.visibilityState !== "hidden";
const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

/**
 * The DM with `chat` in `only` (a Space id) or all its Spaces: each Space's
 * DM entries read and followed (`thread.read {with, followMs}`), marked read
 * while the page is visible, and folded like the thread.
 */
export function useAgentDm(chat: AgentChat | null, only: string | null): AgentDm {
  const { data } = useBridge();
  const [entries, setEntries] = useState<Record<string, ThreadEntry[]>>({});
  const [status, setStatus] = useState<AgentDm["status"]>("loading");
  const [error, setError] = useState<string | null>(null);
  const [me, setMe] = useState<string | null>(null);
  const [readSeq, setReadSeq] = useState<Record<string, number | null>>({});
  const targets = (chat?.spaces ?? []).filter((s) => !only || s.spaceId === only);
  const sig = targets.map((t) => `${t.spaceId}#${t.handle}`).join("\n");
  const merge = useCallback((spaceId: string, more: readonly ThreadEntry[]) => {
    if (!more.length) return;
    setEntries((was) => {
      const by = new Map((was[spaceId] ?? []).map((e) => [e.seq, e]));
      for (const e of more) by.set(e.seq, e);
      return { ...was, [spaceId]: [...by.values()].sort((a, b) => a.seq - b.seq) };
    });
  }, []);
  useEffect(() => {
    setEntries({});
    setReadSeq({});
    setStatus("loading");
    setError(null);
    if (!data || !targets.length) return;
    let alive = true;
    let ready = 0;
    for (const t of targets) {
      void (async () => {
        let cursor = 0;
        try {
          const first = await data.call("thread.read", { spaceId: t.spaceId, with: t.handle, limit: 100, markRead: visible() });
          if (!alive) return;
          cursor = first.cursor;
          merge(t.spaceId, first.entries);
          setMe((m) => m ?? first.me);
          setReadSeq((r) => ({ ...r, [t.spaceId]: first.readSeq ?? readSeqFrom(first.entries, first.unread, first.me) }));
          if (++ready === targets.length) setStatus("ready");
        } catch (e) {
          if (!alive) return;
          setError(e instanceof Error ? e.message : String(e));
          setStatus("error");
          return;
        }
        let backoff = 1_000;
        while (alive) {
          try {
            const page = await data.call("thread.read", { spaceId: t.spaceId, with: t.handle, after: cursor, followMs: THREAD_FOLLOW_MAX_MS, markRead: visible() });
            if (!alive) return;
            cursor = Math.max(cursor, page.cursor);
            merge(t.spaceId, page.entries);
            backoff = 1_000;
            if (!page.entries.length) await sleep(2_000);
          } catch {
            if (!alive) return;
            await sleep(backoff);
            backoff = Math.min(30_000, backoff * 2);
          }
        }
      })();
    }
    return () => {
      alive = false;
    };
    // `targets` follows `sig`.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [data, sig, merge]);

  // Fold each Space's DM like its thread, then interleave by time.
  const items: DmItem[] = [];
  for (const t of targets) {
    for (const item of foldThread(entries[t.spaceId] ?? [], { readSeq: targets.length === 1 ? readSeq[t.spaceId] : null, me })) {
      const tsMs = item.type === "unread" ? 0 : item.entry.tsMs;
      items.push({ item, spaceId: t.spaceId, tsMs });
    }
  }
  if (targets.length > 1) items.sort((a, b) => a.tsMs - b.tsMs);

  const send = useCallback(
    async (spaceId: string, text: string) => {
      const t = chat?.spaces.find((s) => s.spaceId === spaceId);
      if (!data || !t || !text.trim()) return;
      // Sending is posting: an @mention of the agent is its task in that Space.
      const body = new RegExp(`(^|\\s)@${t.handle}\\b`, "i").test(text) ? text.trim() : `@${t.handle} ${text.trim()}`;
      const entry = await data.call("thread.post", { spaceId, text: body });
      if (entry) merge(spaceId, [entry]);
      setReadSeq((r) => ({ ...r, [spaceId]: null }));
    },
    [data, chat, merge],
  );
  const answer = useCallback(
    async (spaceId: string, approval: number, approve: boolean) => {
      if (!data) return;
      const entry = await data.call("thread.post", { spaceId, text: approve ? "Approved" : "Denied", answer: { approval, approve } });
      if (entry) merge(spaceId, [entry]);
    },
    [data, merge],
  );
  return { status: targets.length ? status : "ready", error, items, entries, me, send, answer };
}
