// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The Space thread on the Space page (CUA-2155): the entries folded into
 * what the page draws (`foldThread`), and `useSpaceThread`, which reads the
 * thread with a cursor and long-polls for more (`thread.read` with
 * `followMs`), posts, answers approvals and lists the members.
 *
 * The thread reads like a chatroom (CUA-2640): one row per entry, in order,
 * each with its author and time. Nothing nests. Folding (nothing in the log
 * is rewritten, DESIGN §3.2):
 * - a message with `task` is a task; its state is the last `task_update`
 *   for it (`queued` until one arrives), kept for the logic that needs it
 *   (Retry, the unread divider) and shown nowhere on the message: the agent's
 *   status and its reply tell how it is going;
 * - a reply is a message like any other, in sequence; `quote` names the
 *   message it answers, which the page shows only when that message is not
 *   right above it;
 * - an `approval` shows its Approve / Deny until an `answer` names it;
 * - `system` entries, and task updates that carry a note, are one-line muted
 *   notes, in sequence;
 * - the unread divider sits before the first entry after the reader's
 *   cursor as it was when the thread opened, that someone else wrote.
 */

import { useCallback, useEffect, useRef, useState } from "react";

import { useBridge } from "./hooks";
import type { ShareEntry } from "./contracts/share";
import {
  isMissingThreadTool,
  threadOpAvailable,
  THREAD_FOLLOW_MAX_MS,
  type AgentMember,
  type AgentRights,
  type LiveAgent,
  type TaskState,
  type ThreadAuthor,
  type ThreadEntry,
  type ThreadRole,
  type ThreadView,
} from "./ops/thread";

export interface ThreadAnswer {
  approve: boolean;
  by: ThreadAuthor;
  tsMs: number;
  note: string | null;
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
  | { type: "unread"; key: string };

/** The thread's entries (in any order, duplicates allowed) as the page draws them: one row each, in sequence. */
export function foldThread(entries: readonly ThreadEntry[], opts: { readSeq?: number | null; me?: string | null } = {}): ThreadItem[] {
  const sorted = dedupe(entries);
  const items: ThreadItem[] = [];
  const bySeq = new Map<number, ThreadEntry>(sorted.map((e) => [e.seq, e]));
  const tasks = new Map<number, Extract<ThreadItem, { type: "task" }>>();
  const approvals = new Map<number, Extract<ThreadItem, { type: "approval" | "join_request" }>>();
  for (const e of sorted) {
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
        a.answer = { approve: e.answer.approve, by: e.author, tsMs: e.tsMs, note: e.answer.note };
        continue;
      }
      items.push({ type: "system", key: `s${e.seq}`, entry: e, text: `${e.author.name} ${e.answer.approve ? "approved" : "denied"} a request` });
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
    items.push({
      type: "message",
      key: `m${e.seq}`,
      entry: e,
      quote: parent ? { seq: parent.seq, author: parent.author, text: parent.text } : null,
      taskText: parent?.task ? parent.text : null,
    });
  }
  const readSeq = opts.readSeq;
  if (readSeq == null) return items;
  const first = items.findIndex((i) => i.type !== "unread" && i.type !== "system" && unreadIn(i, readSeq, opts.me ?? null));
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

/* ---- The hook ------------------------------------------------------------------------- */

export type SpaceThreadStatus = "loading" | "ready" | "unsupported" | "error";

export interface SpaceThread {
  /** `unsupported`: the daemon has no thread; the page keeps its old layout. */
  status: SpaceThreadStatus;
  items: ThreadItem[];
  entries: ThreadEntry[];
  members: AgentMember[];
  /** Who the Space is shared with (`sharing.list`). */
  shares: ShareEntry[];
  /** Who is at work now and at what stage ("starting", "thinking", "using Bash"): the live rows at the end of the thread. */
  live: LiveAgent[];
  /** The reader's own author id, when known. */
  me: string | null;
  /** The reader's role (a guest reads only); null from a daemon that does not say (the owner's). */
  role: ThreadRole | null;
  /** The last read failed (the entries already shown stay). */
  error: string | null;
  /** Older entries exist above the first one read. */
  hasMoreBefore: boolean;
  /** Reads the page before the oldest entry shown. */
  loadEarlier(): Promise<void>;
  /** Posts `text`; `view` says what the person is looking at in the Space (the main view's window). */
  post(text: string, view?: ThreadView | null): Promise<ThreadEntry | null>;
  answer(approval: number, approve: boolean): Promise<void>;
  /** Changes who can task, control and approve an agent (owners and admins). */
  setRights(name: string, rights: Partial<AgentRights>): Promise<void>;
  /** Removes an agent member (its key stops at once); null where the host can't yet. The daemon refuses anyone but the owner, admins and the agent's owner. */
  removeAgent: ((name: string) => Promise<void>) | null;
}

/**
 * Streaming text arrives as whole-text snapshots; they are drawn at most this often (ms), the latest
 * one winning, so a fast stream does not re-render the thread for every snapshot. A stage change, an
 * agent arriving or leaving shows at once.
 */
export const LIVE_COALESCE_MS = 50;

/** The live rows without their streaming text: what makes a change worth drawing at once. */
const liveShape = (agents: readonly LiveAgent[]) => agents.map((a) => `${a.agent}=${a.stage}`).join(";");

const FIRST_PAGE = 100;
const MEMBERS_POLL_MS = 5_000;
const MAX_BACKOFF_MS = 30_000;

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));
const visible = () => typeof document === "undefined" || document.visibilityState !== "hidden";

/**
 * One Space's thread while mounted. `spaceId` null: nothing is read. The
 * first page is the latest entries; after that it long-polls from the
 * cursor and marks what the reader saw as read while the page is visible.
 */
export function useSpaceThread(spaceId: string | null): SpaceThread {
  const { data } = useBridge();
  const [status, setStatus] = useState<SpaceThreadStatus>("loading");
  const [entries, setEntries] = useState<ThreadEntry[]>([]);
  const [readSeq, setReadSeq] = useState<number | null>(null);
  const [me, setMe] = useState<string | null>(null);
  const [role, setRole] = useState<ThreadRole | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [members, setMembers] = useState<AgentMember[]>([]);
  const [shares, setShares] = useState<ShareEntry[]>([]);
  const [live, setLiveNow] = useState<LiveAgent[]>([]);
  const [hasMoreBefore, setHasMoreBefore] = useState(false);
  // The snapshot waiting for its turn, and when it is drawn.
  const livePending = useRef<LiveAgent[] | null>(null);
  const liveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const liveShown = useRef<LiveAgent[]>([]);
  const setLive = useCallback((agents: LiveAgent[]) => {
    if (liveTimer.current) {
      clearTimeout(liveTimer.current);
      liveTimer.current = null;
    }
    livePending.current = null;
    liveShown.current = agents;
    setLiveNow(agents);
  }, []);
  /** A snapshot of the live rows: drawn now when a stage changed, else within LIVE_COALESCE_MS (the latest wins). */
  const pushLive = useCallback(
    (agents: LiveAgent[]) => {
      if (liveShape(agents) !== liveShape(liveShown.current)) return setLive(agents);
      livePending.current = agents;
      if (liveTimer.current) return;
      liveTimer.current = setTimeout(() => {
        liveTimer.current = null;
        if (livePending.current) setLive(livePending.current);
      }, LIVE_COALESCE_MS);
    },
    [setLive],
  );
  const loadingEarlier = useRef(false);
  const merge = useCallback((more: readonly ThreadEntry[]) => {
    if (!more.length) return;
    setEntries((was) => dedupe([...was, ...more]));
  }, []);
  // The members change as tasks move; read them again soon after.
  const membersDue = useRef<() => void>(() => {});

  useEffect(() => {
    setStatus("loading");
    setEntries([]);
    setReadSeq(null);
    setMe(null);
    setRole(null);
    setError(null);
    setMembers([]);
    setShares([]);
    setLive([]);
    setHasMoreBefore(false);
    if (!spaceId || !data) return;
    let alive = true;
    let cursor = 0;
    // The stage key last seen: the follow also returns when it changes.
    let liveKey = "";
    // The same without what is said: what moves the members list.
    let liveShapeSeen = "";
    const markRead = () => {
      if (!alive || !visible() || cursor <= 0) return;
      // Re-reads the last entry with mark_read: the daemon moves the
      // reader's cursor to it. The divider stays where it was.
      void data.call("thread.read", { spaceId, after: cursor - 1, limit: 1, markRead: true }).catch(() => {});
    };
    const onVisible = () => {
      if (visible()) markRead();
    };
    document.addEventListener("visibilitychange", onVisible);
    void (async () => {
      let backoff = 1_000;
      try {
        const first = await data.call("thread.read", { spaceId, limit: FIRST_PAGE });
        if (!alive) return;
        cursor = first.cursor;
        liveKey = first.live.key;
        setLive(first.live.agents);
        merge(first.entries);
        setMe(first.me);
        setRole(first.role);
        setHasMoreBefore(first.hasMoreBefore);
        setReadSeq(first.readSeq ?? readSeqFrom(first.entries, first.unread, first.me));
        setStatus("ready");
        markRead();
      } catch (e) {
        if (!alive) return;
        if (isMissingThreadTool(e)) {
          setStatus("unsupported");
          return;
        }
        setError(message(e));
        setStatus("error");
      }
      while (alive) {
        try {
          const page = await data.call("thread.read", { spaceId, after: cursor, followMs: THREAD_FOLLOW_MAX_MS, markRead: visible(), liveAfter: liveKey });
          if (!alive) return;
          setStatus("ready");
          setError(null);
          backoff = 1_000;
          cursor = Math.max(cursor, page.cursor);
          // The daemon's key moves with the stages and with what is said; the members only change with a stage.
          const keyMoved = page.live.key !== liveKey;
          const shape = liveShape(page.live.agents);
          const stageMoved = shape !== liveShapeSeen;
          liveShapeSeen = shape;
          liveKey = page.live.key;
          // The stages ride the follow (not the members poll), so a row shows as it changes.
          pushLive(page.live.agents);
          merge(page.entries);
          if (page.entries.some((e) => e.kind !== "message") || stageMoved) membersDue.current();
          // A daemon that answers a follow at once (no long poll) is read every 2 s.
          if (!page.entries.length && !keyMoved) await sleep(2_000);
        } catch (e) {
          if (!alive) return;
          if (isMissingThreadTool(e)) {
            setStatus("unsupported");
            return;
          }
          setError(message(e));
          await sleep(backoff);
          backoff = Math.min(MAX_BACKOFF_MS, backoff * 2);
        }
      }
    })();
    return () => {
      alive = false;
      document.removeEventListener("visibilitychange", onVisible);
      if (liveTimer.current) clearTimeout(liveTimer.current);
      liveTimer.current = null;
    };
  }, [data, spaceId, merge, setLive, pushLive]);

  // Members (`agent_members`) every few seconds, and who the Space is shared with once.
  useEffect(() => {
    if (!spaceId || !data || status !== "ready") return;
    let alive = true;
    const read = () =>
      void data.call("thread.members", { spaceId }).then(
        (r) => alive && setMembers(r.members),
        () => {},
      );
    membersDue.current = read;
    read();
    const t = setInterval(read, MEMBERS_POLL_MS);
    void data.call("sharing.list", { spaceId }).then(
      (s) => alive && setShares(s),
      () => {},
    );
    return () => {
      alive = false;
      membersDue.current = () => {};
      clearInterval(t);
    };
  }, [data, spaceId, status]);

  const loadEarlier = useCallback(async () => {
    const oldest = entries[0]?.seq;
    if (!spaceId || !data || !hasMoreBefore || oldest === undefined || loadingEarlier.current) return;
    loadingEarlier.current = true;
    try {
      const page = await data.call("thread.read", { spaceId, before: oldest, limit: FIRST_PAGE });
      merge(page.entries);
      setHasMoreBefore(page.hasMoreBefore && page.entries.length > 0);
    } catch (e) {
      setError(message(e));
    } finally {
      loadingEarlier.current = false;
    }
  }, [data, spaceId, entries, hasMoreBefore, merge]);

  const post = useCallback(
    async (text: string, view?: ThreadView | null) => {
      if (!spaceId || !data || !text.trim()) return null;
      const entry = await data.call("thread.post", { spaceId, text: text.trim(), ...(view ? { view } : {}) });
      if (entry) merge([entry]);
      // Your own message: everything above it is read; the divider goes.
      setReadSeq(null);
      return entry ?? null;
    },
    [data, spaceId, merge],
  );
  const answer = useCallback(
    async (approval: number, approve: boolean) => {
      if (!spaceId || !data) return;
      const entry = await data.call("thread.post", { spaceId, text: approve ? "Approved" : "Denied", answer: { approval, approve } });
      if (entry) merge([entry]);
    },
    [data, spaceId, merge],
  );

  const setRights = useCallback(
    async (name: string, rights: Partial<AgentRights>) => {
      if (!spaceId || !data) return;
      const m = await data.call("thread.setRights", { spaceId, name, ...rights });
      if (m) setMembers((was) => was.map((x) => (x.name === m.name ? m : x)));
    },
    [data, spaceId],
  );

  const removeAgent = useCallback(
    async (name: string) => {
      if (!spaceId || !data) return;
      await data.call("thread.agentRemove", { spaceId, name });
      // It leaves the members now; its posts stay, and the daemon's note arrives with the next read.
      setMembers((was) => was.filter((x) => x.name !== name));
      membersDue.current();
    },
    [data, spaceId],
  );
  const canRemove = !!data && threadOpAvailable(data.mode, "thread.agentRemove");

  return {
    status,
    items: foldThread(entries, { readSeq, me }),
    entries,
    members,
    shares,
    live,
    me,
    role,
    error,
    hasMoreBefore,
    loadEarlier,
    post,
    answer,
    setRights,
    removeAgent: canRemove ? removeAgent : null,
  };
}

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));
