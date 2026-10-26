// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The Space thread on the Space page (CUA-2155): the entries folded into
 * what the page draws (`foldThread`), and `useSpaceThread`, which reads the
 * thread with a cursor and long-polls for more (`thread.read` with
 * `followMs`), posts, answers approvals and lists the members.
 *
 * Folding (nothing in the log is rewritten, DESIGN §3.2):
 * - a message with `task` is a task; its state is the last `task_update`
 *   for it (`queued` until one arrives), and the replies to it nest under it;
 * - an `approval` shows its Approve / Deny until an `answer` names it;
 * - `system` entries, and task updates that carry a note, are one-line notes;
 *   a note about a task (`task_seq` on the note, or the update's task) shows
 *   inside that task, in order with its replies;
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

export type ThreadItem =
  | { type: "message"; key: string; entry: ThreadEntry }
  | {
      type: "task";
      key: string;
      entry: ThreadEntry;
      state: TaskState;
      session: number | null;
      replies: ThreadEntry[];
      /** Notes about this task (a system note's `task`, an update's note), shown inside it in order with the replies. */
      notes: { entry: ThreadEntry; text: string }[];
    }
  | { type: "approval"; key: string; entry: ThreadEntry; answer: ThreadAnswer | null }
  /** Someone's agent asks to join (CUA-2264): answered like an approval. */
  | { type: "join_request"; key: string; entry: ThreadEntry; answer: ThreadAnswer | null }
  | { type: "system"; key: string; entry: ThreadEntry; text: string }
  | { type: "unread"; key: string };

/** The thread's entries (in any order, duplicates allowed) as the page draws them. */
export function foldThread(entries: readonly ThreadEntry[], opts: { readSeq?: number | null; me?: string | null } = {}): ThreadItem[] {
  const sorted = dedupe(entries);
  const items: ThreadItem[] = [];
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
      if (u?.note) {
        if (task) task.notes.push({ entry: e, text: u.note });
        else items.push({ type: "system", key: `s${e.seq}`, entry: e, text: u.note });
      }
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
      const task = e.noteFor !== null ? tasks.get(e.noteFor) : undefined;
      if (task) {
        task.notes.push({ entry: e, text: e.text });
        continue;
      }
      items.push({ type: "system", key: `s${e.seq}`, entry: e, text: e.text });
      continue;
    }
    if (e.replyTo !== null) {
      const parent = tasks.get(e.replyTo);
      if (parent) {
        parent.replies.push(e);
        continue;
      }
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
    if (e.task) {
      const item = { type: "task" as const, key: `t${e.seq}`, entry: e, state: "queued" as TaskState, session: e.task.session, replies: [] as ThreadEntry[], notes: [] as { entry: ThreadEntry; text: string }[] };
      tasks.set(e.seq, item);
      items.push(item);
      continue;
    }
    items.push({ type: "message", key: `m${e.seq}`, entry: e });
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
  const fresh = (e: ThreadEntry) => e.seq > readSeq && e.author.id !== me;
  if (fresh(item.entry)) return true;
  return item.type === "task" && item.replies.some(fresh);
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
  const [hasMoreBefore, setHasMoreBefore] = useState(false);
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
    setHasMoreBefore(false);
    if (!spaceId || !data) return;
    let alive = true;
    let cursor = 0;
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
          const page = await data.call("thread.read", { spaceId, after: cursor, followMs: THREAD_FOLLOW_MAX_MS, markRead: visible() });
          if (!alive) return;
          setStatus("ready");
          setError(null);
          backoff = 1_000;
          cursor = Math.max(cursor, page.cursor);
          merge(page.entries);
          if (page.entries.some((e) => e.kind !== "message")) membersDue.current();
          // A daemon that answers a follow at once (no long poll) is read every 2 s.
          if (!page.entries.length) await sleep(2_000);
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
    };
  }, [data, spaceId, merge]);

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
