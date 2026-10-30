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

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import type { CoreClient } from "./core";
import { chatsPollMs, forEachLimit, jittered, MissCache } from "../lib/poll-plan";
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

export type ChatTone = "you" | "working" | "idle" | "asleep" | "failed" | "not-connected" | "creating" | "stopped";

/** A run of this device's that isn't an agent in a Space (yet): its job waiting for a Space being made, an agent not connected, a start that failed. */
export interface ChatAttempt {
  id: string;
  spaceId: string;
  spaceName: string;
  harness: string;
  label: string;
  job: string;
  atMs: number;
  state: "creating" | "not-connected" | "failed" | "stopped";
  note: string | null;
  connectLabel: string | null;
  app: string | null;
}

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
  /** `agent` (in a Space's thread) or `attempt` (a run of this device's). */
  kind: "agent" | "attempt";
  /** What it was asked to do, or is doing. */
  job: string | null;
  /** Where it is (an attempt has no member there yet). */
  places: { spaceId: string; spaceName: string }[];
  /** The step that moves it on: Connect or Try again. */
  action: { id: "connect" | "retry"; label: string } | null;
  app: string | null;
  attempt: string | null;
}

/** One agent across Spaces: the same owner, harness and name. Without an identity, its handle in its Space. */
export function chatKey(spaceId: string, m: AgentMember): string {
  const i = m.identity;
  if (!i) return `${spaceId}/${m.name}`;
  // The same rule as `identityKey` on the thread's card (its Activity link opens this row).
  return `${i.owner.id}/${i.harness.id || "other"}/${i.displayName.trim().toLowerCase()}`;
}

/**
 * Each Space's members (with `overview`) and this device's unfinished runs as
 * one list: the app core's `agentList.view` (`agent_list.rs`) decides the
 * rows, their order and every word; this only hangs each Space's member
 * records back on the rows. The Agents page and Home both use it.
 */
export function mergeChats(core: CoreClient, perSpace: readonly { space: Space; role: ThreadRole | null; members: readonly AgentMember[] }[], attempts: readonly ChatAttempt[] = []): AgentChat[] {
  const input = {
    spaces: perSpace.map(({ space, members }) => ({
      spaceId: space.id,
      spaceName: space.name,
      members: members.map((m) => ({
        handle: m.name,
        harness: m.identity?.harness.id || m.harness,
        displayName: m.identity?.displayName ?? "",
        ownerId: m.identity?.owner.id ?? "",
        status: m.status,
        activity: m.activity,
        unread: m.dm?.unread ?? 0,
        lastMs: m.dm?.last?.tsMs ?? 0,
        needsMs: (m.needsYou ?? []).map((n) => n.tsMs),
        pending: m.identity?.state === "pending",
      })),
    })),
    attempts,
  };
  const view = core.tryCall<{ rows: AgentListRow[] }>("agentList.view", { input: input as unknown as Record<string, unknown> });
  if (!view) return [];

  return view.rows.map((r) => {
    const spaces: AgentChatSpace[] = r.kind === "agent"
      ? r.places.flatMap((pl) => {
          // A Space may come in more than one part: the one that has this member.
          const p = perSpace.find((x) => x.space.id === pl.spaceId && x.members.some((m) => m.name === pl.handle));
          const member = p?.members.find((m) => m.name === pl.handle);
          return p && member ? [{ spaceId: p.space.id, spaceName: p.space.name, os: p.space.os, handle: pl.handle, member, role: p.role }] : [];
        })
      : [];
    const needsYou: ChatNeed[] = spaces.flatMap((sp) => (sp.member.needsYou ?? []).map((n) => ({ ...n, spaceId: sp.spaceId, spaceName: sp.spaceName }))).sort((a, b) => b.tsMs - a.tsMs);
    return {
      key: r.key,
      label: r.label,
      identity: spaces[0]?.member.identity ?? null,
      harness: r.harness,
      spaces,
      unread: r.unread,
      needsYou,
      tone: r.tone,
      line: r.line,
      lastMs: r.lastMs,
      kind: r.kind,
      job: r.job,
      places: r.places.map((p) => ({ spaceId: p.spaceId, spaceName: p.spaceName })),
      action: r.action,
      app: r.app,
      attempt: r.attempt,
    };
  });
}

/** A row of the core's list (`agent_list::AgentListRow`). */
interface AgentListRow {
  key: string;
  kind: "agent" | "attempt";
  label: string;
  harness: string;
  tone: ChatTone;
  line: string;
  job: string | null;
  lastMs: number;
  unread: number;
  places: { spaceId: string; spaceName: string; handle: string }[];
  action: { id: "connect" | "retry"; label: string } | null;
  app: string | null;
  attempt: string | null;
}

/** The Agents tab's base poll; it slows as the account's Spaces grow (`chatsPollMs`). */
export const CHATS_POLL_MS = 5_000;
/** Spaces' threads read at once, at most. */
const CHATS_PARALLEL = 16;
/** A Space that has not answered in this long stops holding its read slot (it keeps going, and fills in when it answers). */
export const CHATS_SLOT_MS = 4_000;
/** How long a Space with no thread is left alone. */
const MISS_TTL_MS = 60_000;
/** The newest messages read first in each Space; older ones come on request. */
export const DM_FIRST_PAGE = 40;
/** The list as of the last visit, drawn at once on the next one. */
const CACHE_KEY = "cua.agents.lastKnown.v1";
/** Answers the page redraws on at most this often while a pass fills in. */
const FLUSH_MS = 60;

export interface AgentChats {
  /** null while there is nothing to show yet (no earlier list, no run of this device's, no Space has answered). */
  chats: AgentChat[] | null;
  /** Spaces with a thread (where an agent can be added). */
  spaces: Space[];
  /** Your author id, as the threads say it. */
  me: string | null;
  /** Spaces still to answer in this pass (the list draws what is known meanwhile). */
  loading: number;
  /** Read again now (after adding or removing an agent). */
  refresh(): void;
}

/** What one Space answered: its members with their DM summaries, and your role there. */
export interface SpaceRead {
  role: ThreadRole | null;
  members: AgentMember[];
  me: string | null;
}

function loadLastKnown(): Record<string, SpaceRead> {
  try {
    const raw = typeof localStorage === "undefined" ? null : localStorage.getItem(CACHE_KEY);
    const v = raw ? (JSON.parse(raw) as unknown) : null;
    return v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, SpaceRead>) : {};
  } catch {
    return {};
  }
}

function saveLastKnown(reads: ReadonlyMap<string, SpaceRead>): void {
  try {
    localStorage.setItem(CACHE_KEY, JSON.stringify(Object.fromEntries(reads)));
  } catch {
    // No storage (a private window, a full disk): the list just draws after its first answers.
  }
}

/** Every agent in every Space you can see, as the Agents tab's list. */
export function useAgentChats(attempts: readonly ChatAttempt[] = []): AgentChats {
  const { data, core } = useBridge();
  const { data: spaces } = useSpaces();
  // What each Space last said: the last visit's answers first, then each Space's own as it comes. A Space
  // that does not answer keeps its last answer (a machine that is off still lists its agents).
  const reads = useRef<Map<string, SpaceRead> | null>(null);
  reads.current ??= new Map(Object.entries(loadLastKnown()));
  const [version, setVersion] = useState(0);
  const [settled, setSettled] = useState(false);
  const [pending, setPending] = useState(0);
  const [round, setRound] = useState(0);
  const list = (spaces ?? []).filter((s) => !s.id.startsWith("pending:"));
  const ids = list.map((s) => s.id).join("\n");
  // Not asked again for a minute: a Space with no thread (a stopped machine, an old daemon) stays that way.
  const misses = useRef(new MissCache(MISS_TTL_MS));
  const inflight = useRef(new Set<string>());
  useEffect(() => {
    if (!data || spaces === undefined) return;
    let alive = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let flushTimer: ReturnType<typeof setTimeout> | undefined;
    const known = new Set(list.map((s) => s.id));
    misses.current.prune(known);
    for (const id of [...reads.current!.keys()]) if (!known.has(id)) reads.current!.delete(id);
    // A pass's answers redraw the page together, not one by one.
    const flush = () => {
      if (flushTimer !== undefined) return;
      flushTimer = setTimeout(() => {
        flushTimer = undefined;
        if (!alive) return;
        saveLastKnown(reads.current!);
        setVersion((v) => v + 1);
      }, FLUSH_MS);
    };
    const readOne = async (space: Space) => {
      inflight.current.add(space.id);
      try {
        const [{ members }, page] = await Promise.all([data.call("thread.members", { spaceId: space.id, overview: true }), data.call("thread.read", { spaceId: space.id, limit: 1 })]);
        reads.current!.set(space.id, { role: page.role, members, me: page.me });
      } catch (e) {
        // No thread there (an old daemon): not in the list, and not asked again for a while.
        if (isMissingThreadTool(e)) {
          misses.current.add(space.id);
          reads.current!.delete(space.id);
        } else console.debug("agents: couldn't read", space.id, e);
      } finally {
        inflight.current.delete(space.id);
        if (alive) {
          setPending((n) => Math.max(0, n - 1));
          flush();
        }
      }
    };
    const pass = async () => {
      const asked = list.filter((s) => !misses.current.has(s.id) && !inflight.current.has(s.id));
      setPending(asked.length);
      // Spaces that answered last time go first: the slow ones are last in line, never in front.
      asked.sort((x, y) => Number(reads.current!.has(y.id)) - Number(reads.current!.has(x.id)));
      await forEachLimit(asked, CHATS_PARALLEL, CHATS_SLOT_MS, readOne);
      if (alive) {
        setSettled(true);
        flush();
      }
    };
    // One pass at a time, the next after the last one's slots end: slower as the
    // account grows (`chatsPollMs`), spread so pages don't ask together, and
    // paused while the page is hidden (it reads at once on coming back).
    const loop = async () => {
      if (visible()) await pass();
      if (alive) timer = setTimeout(() => void loop(), jittered(chatsPollMs(list.length)));
    };
    const onVisible = () => {
      if (!visible() || !alive) return;
      clearTimeout(timer);
      void loop();
    };
    document.addEventListener("visibilitychange", onVisible);
    void loop();
    return () => {
      alive = false;
      clearTimeout(timer);
      clearTimeout(flushTimer);
      document.removeEventListener("visibilitychange", onVisible);
    };
    // `list` follows `ids`.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [data, ids, round, spaces === undefined]);
  // One list, with this device's unfinished runs (the core's order and words), made again only when an
  // answer, the Spaces or the runs change.
  const merged = useMemo(() => {
    const perSpace = list.flatMap((space) => {
      const r = reads.current!.get(space.id);
      return r ? [{ space, role: r.role, members: r.members }] : [];
    });
    return { perSpace, chats: spaces === undefined ? [] : mergeChats(core, perSpace, attempts) };
    // `list` follows `spaces`; `reads` is read when `version` says it changed.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [version, spaces, attempts, core]);
  const chats = merged.chats.length === 0 && !settled ? null : merged.chats;
  const withThread = useMemo(() => merged.perSpace.map((p) => p.space), [merged]);
  const me = useMemo(() => [...reads.current!.values()].find((r) => r.me)?.me ?? null, [merged]);
  return { chats, spaces: withThread, me, loading: pending, refresh: useCallback(() => setRound((r) => r + 1), []) };
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
  /** Spaces whose first page has not come yet (the others' messages show meanwhile). */
  pending: number;
  /** Older messages exist in some Space than are shown. */
  hasMore: boolean;
  /** Reads the page before the oldest shown, in each Space that has more. */
  loadOlder(): Promise<void>;
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
  const [pending, setPending] = useState(0);
  const [more, setMore] = useState<Record<string, boolean>>({});
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
  // The oldest seq shown per Space, for paging back (read at the click, not in a render).
  const oldest = useRef<Record<string, number>>({});
  useEffect(() => {
    oldest.current = {};
    for (const [id, list] of Object.entries(entries)) if (list.length) oldest.current[id] = list[0]!.seq;
  }, [entries]);
  useEffect(() => {
    setEntries({});
    setReadSeq({});
    setMore({});
    setStatus("loading");
    setError(null);
    setPending(targets.length);
    if (!data || !targets.length) return;
    let alive = true;
    let ok = 0;
    let settled = 0;
    // The first page of each Space shows as it comes: one Space's slow answer never holds the others'.
    const settle = (good: boolean) => {
      settled++;
      if (good) ok++;
      setPending(targets.length - settled);
      setStatus(ok > 0 ? "ready" : settled === targets.length ? "error" : "loading");
    };
    for (const t of targets) {
      void (async () => {
        let cursor = 0;
        try {
          const first = await data.call("thread.read", { spaceId: t.spaceId, with: t.handle, limit: DM_FIRST_PAGE, markRead: visible() });
          if (!alive) return;
          cursor = first.cursor;
          merge(t.spaceId, first.entries);
          setMe((m) => m ?? first.me);
          setMore((m) => ({ ...m, [t.spaceId]: first.hasMoreBefore }));
          setReadSeq((r) => ({ ...r, [t.spaceId]: first.readSeq ?? readSeqFrom(first.entries, first.unread, first.me) }));
          settle(true);
        } catch (e) {
          if (!alive) return;
          setError(e instanceof Error ? e.message : String(e));
          settle(false);
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

  const loadOlder = useCallback(async () => {
    if (!data) return;
    await Promise.all(
      targets
        .filter((t) => more[t.spaceId] && oldest.current[t.spaceId] !== undefined)
        .map(async (t) => {
          try {
            const page = await data.call("thread.read", { spaceId: t.spaceId, with: t.handle, before: oldest.current[t.spaceId], limit: DM_FIRST_PAGE });
            merge(t.spaceId, page.entries);
            setMore((m) => ({ ...m, [t.spaceId]: page.hasMoreBefore }));
          } catch (e) {
            setError(e instanceof Error ? e.message : String(e));
          }
        }),
    );
    // `targets` follows `sig`.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [data, sig, more, merge]);

  // Fold each Space's DM like its thread, then interleave by time: made again only when the entries,
  // the read positions or the Spaces shown change, not on every render of the page.
  const items = useMemo(() => {
    const out: DmItem[] = [];
    for (const t of targets) {
      for (const item of foldThread(entries[t.spaceId] ?? [], { readSeq: targets.length === 1 ? readSeq[t.spaceId] : null, me })) {
        const tsMs = item.type === "unread" ? 0 : item.entry.tsMs;
        out.push({ item, spaceId: t.spaceId, tsMs });
      }
    }
    if (targets.length > 1) out.sort((a, b) => a.tsMs - b.tsMs);
    return out;
    // `targets` follows `sig`.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [entries, readSeq, me, sig]);
  const hasMore = targets.some((t) => more[t.spaceId]);

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
  return { status: targets.length ? status : "ready", error, items, entries, me, pending, hasMore, loadOlder, send, answer };
}
