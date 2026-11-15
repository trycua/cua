// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { AppState } from "react-native";

import { foldThread, readSeqFrom, type ThreadItem } from "../data/thread/fold";
import { isMissingThreadTool, type AgentMember, type LiveAgent, type ThreadEntry, type ThreadView } from "../data/thread/protocol";
import { mergeToolRows, type ToolGroup } from "../data/thread/phone-protocol";
import type { Backend, SpaceList } from "../data/types";
import type { OutboxRow } from "../core/phone";
import { isNetworkError } from "../store/network";
import { now as clockNow } from "../ui/clock";
import { useOutbox, useStores } from "./stores";

const BackendContext = createContext<Backend | null>(null);

export function BackendProvider({ backend, children }: { backend: Backend; children: ReactNode }) {
  return <BackendContext.Provider value={backend}>{children}</BackendContext.Provider>;
}

export function useBackend(): Backend {
  const b = useContext(BackendContext);
  if (!b) throw new Error("useBackend outside BackendProvider");
  return b;
}

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));
const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

/** True while the app is in the foreground: nothing polls in the background. */
export function useForeground(): boolean {
  const [on, setOn] = useState(AppState.currentState !== "background");
  useEffect(() => {
    const sub = AppState.addEventListener("change", (s) => setOn(s !== "background"));
    return () => sub.remove();
  }, []);
  return on;
}

export interface SpaceListState {
  status: "loading" | "ready" | "error";
  list: SpaceList | null;
  error: string | null;
  /** Pull to refresh. */
  refreshing: boolean;
  refresh(): Promise<void>;
  /** What is shown is the copy saved on this phone (when it was saved), not yet confirmed by the machines. */
  cached: { savedMs: number } | null;
  /** When the machines last answered (this launch). */
  syncedMs: number | null;
}

const LIST_POLL_MS = 6_000;

export function useSpaceList(): SpaceListState {
  const backend = useBackend();
  const stores = useStores();
  const foreground = useForeground();
  const [list, setList] = useState<SpaceList | null>(null);
  const [status, setStatus] = useState<SpaceListState["status"]>("loading");
  const [error, setError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [cached, setCached] = useState<{ savedMs: number } | null>(null);
  const [syncedMs, setSyncedMs] = useState<number | null>(null);
  const live = useRef(false);

  const load = useCallback(async () => {
    try {
      const l = await backend.listSpaces();
      live.current = true;
      setList(l);
      setCached(null);
      setSyncedMs(clockNow());
      setStatus("ready");
      setError(null);
      stores?.outbox.setOnline(true);
      void stores?.cache.saveSpaces(l);
    } catch (e) {
      setError(message(e));
      if (isNetworkError(e)) stores?.outbox.setOnline(false);
      // What was shown stays; only a first load that fails with nothing saved is an error screen.
      setStatus((s) => (s === "ready" ? "ready" : "error"));
    }
  }, [backend, stores]);

  useEffect(() => {
    live.current = false;
    setStatus("loading");
    setList(null);
    setCached(null);
    // The last list this phone saved is on screen at once; the machines then confirm or correct it.
    let alive = true;
    void stores?.cache.loadSpaces().then((c) => {
      if (!alive || !c || live.current) return;
      setList(c.list);
      setCached({ savedMs: c.savedMs });
      setStatus("ready");
    });
    return () => {
      alive = false;
    };
  }, [backend, stores]);

  useEffect(() => {
    if (!foreground) return;
    let alive = true;
    void (async () => {
      while (alive) {
        await load();
        await sleep(LIST_POLL_MS);
      }
    })();
    return () => {
      alive = false;
    };
  }, [load, foreground]);

  const refresh = useCallback(async () => {
    setRefreshing(true);
    try {
      await load();
    } finally {
      setRefreshing(false);
    }
  }, [load]);

  return { status, list, error, refreshing, refresh, cached, syncedMs };
}

export interface ThreadState {
  status: "loading" | "ready" | "error";
  items: ThreadItem[];
  entries: ThreadEntry[];
  members: AgentMember[];
  live: LiveAgent[];
  me: string | null;
  role: string | null;
  error: string | null;
  /** The connection to the machine is down: what is shown stays, a thin line says so. */
  offline: boolean;
  /** What is shown came from this phone's saved copy (when it was saved) and the machine has not confirmed it yet. */
  cached: { savedMs: number } | null;
  /** When the machine last answered (this launch); null before the first answer. */
  syncedMs: number | null;
  hasMoreBefore: boolean;
  loadEarlier(): Promise<void>;
  /** Sends a message through the outbox: written down first, delivered exactly once, kept across a lost connection. */
  post(text: string, opts?: { view?: ThreadView | null; replyTo?: number | null; mode?: "send" | "queue" | "steer"; agent?: string | null }): Promise<void>;
  /** Answers an approval (`scope` `space` allows this kind of action for the Space from now on). */
  answer(approval: number, approve: boolean, scope?: "once" | "space"): Promise<void>;
  /** Messages, answers and tasks not yet taken by the machine: waiting, sending or failed, with the core's words. */
  pending: OutboxRow[];
  /** The line over the thread while the phone is offline or messages did not send (the core's words). */
  outboxBanner: string | null;
  retry(id: string): void;
  discard(id: string): void;
  refresh(): Promise<void>;
}

/** Threads already read in this launch: reopening one shows what it had at once, then catches up. */
const memory = new Map<string, { entries: ThreadEntry[]; members: AgentMember[]; me: string | null; role: string | null }>();

/** Forgets the launch's cached threads (sign-out, and tests). */
export function clearThreadMemory(): void {
  memory.clear();
}

const FIRST_PAGE = 100;
const FOLLOW_MS = 20_000;

export function useThread(spaceId: string | null): ThreadState {
  const backend = useBackend();
  const stores = useStores();
  const outbox = useOutbox();
  const foreground = useForeground();
  const [status, setStatus] = useState<ThreadState["status"]>("loading");
  const [entries, setEntries] = useState<ThreadEntry[]>([]);
  const [readSeq, setReadSeq] = useState<number | null>(null);
  const [me, setMe] = useState<string | null>(null);
  const [role, setRole] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [offline, setOffline] = useState(false);
  const [members, setMembers] = useState<AgentMember[]>([]);
  const [live, setLive] = useState<LiveAgent[]>([]);
  const [hasMoreBefore, setHasMoreBefore] = useState(false);
  const [cached, setCached] = useState<{ savedMs: number } | null>(null);
  const [syncedMs, setSyncedMs] = useState<number | null>(null);
  const cursor = useRef(0);
  const synced = useRef(false);

  const merge = useCallback((more: readonly ThreadEntry[]) => {
    if (!more.length) return;
    setEntries((was) => {
      const by = new Map(was.map((e) => [e.seq, e]));
      for (const e of more) by.set(e.seq, e);
      return [...by.values()].sort((a, b) => a.seq - b.seq);
    });
  }, []);

  useEffect(() => {
    const m = spaceId ? memory.get(spaceId) : undefined;
    synced.current = false;
    setStatus(m ? "ready" : "loading");
    setEntries(m?.entries ?? []);
    setReadSeq(null);
    setMembers(m?.members ?? []);
    setMe(m?.me ?? null);
    setRole(m?.role ?? null);
    setLive([]);
    setError(null);
    setOffline(false);
    setCached(null);
    setSyncedMs(null);
    cursor.current = 0;
    // Not read in this launch: the copy saved on this phone is shown at once, labelled, until the machine answers.
    let alive = true;
    if (!m && spaceId && stores) {
      void stores.cache.loadThread(spaceId).then((c) => {
        if (!alive || !c || synced.current) return;
        setEntries(c.entries);
        setMembers(c.members);
        setMe(c.me);
        setRole(c.role);
        setReadSeq(c.readSeq);
        setCached({ savedMs: c.savedMs });
        setStatus("ready");
      });
    }
    return () => {
      alive = false;
    };
  }, [spaceId, stores]);

  useEffect(() => {
    if (spaceId && entries.length) memory.set(spaceId, { entries: entries.slice(-200), members, me, role });
  }, [spaceId, entries, members, me, role]);

  // What the machine confirmed is saved on this phone (sealed), for the next launch and for offline.
  useEffect(() => {
    if (spaceId && stores && synced.current && !cached && entries.length) stores.cache.saveThread(spaceId, { entries, members, me, role, readSeq });
  }, [spaceId, stores, entries, members, me, role, readSeq, cached]);

  useEffect(() => {
    if (!spaceId || !foreground) return;
    let alive = true;
    void (async () => {
      let backoff = 1_000;
      let liveKey = "";
      let first = true;
      while (alive) {
        try {
          if (first) {
            const p = await backend.readThread(spaceId, { limit: FIRST_PAGE });
            if (!alive) return;
            cursor.current = p.cursor;
            liveKey = p.live.key;
            synced.current = true;
            setLive(p.live.agents);
            merge(p.entries);
            setCached(null);
            setMe(p.me);
            setRole(p.role);
            setHasMoreBefore(p.hasMoreBefore);
            setReadSeq(p.readSeq ?? readSeqFrom(p.entries, p.unread, p.me));
            setOffline(false);
            setError(null);
            setStatus("ready");
            setSyncedMs(clockNow());
            outbox?.setOnline(true);
            first = false;
            void backend.readThread(spaceId, { after: Math.max(0, p.cursor - 1), limit: 1, markRead: true }).catch(() => {});
            backend.members(spaceId).then(
              (m) => alive && setMembers(m),
              () => {},
            );
          } else {
            const p = await backend.readThread(spaceId, { after: cursor.current, followMs: FOLLOW_MS, liveAfter: liveKey, markRead: true });
            if (!alive) return;
            setOffline(false);
            setError(null);
            setSyncedMs(clockNow());
            outbox?.setOnline(true);
            backoff = 1_000;
            cursor.current = Math.max(cursor.current, p.cursor);
            const moved = p.live.key !== liveKey;
            liveKey = p.live.key;
            setLive(p.live.agents);
            merge(p.entries);
            if (p.entries.some((e) => e.kind !== "message") || moved)
              backend.members(spaceId).then(
                (m) => alive && setMembers(m),
                () => {},
              );
            if (!p.entries.length && !moved) await sleep(1_500);
          }
        } catch (e) {
          if (!alive) return;
          setError(message(e));
          if (isNetworkError(e)) outbox?.setOnline(false);
          // A first read that fails with nothing to show is an error screen; with a saved copy it is the offline line over it.
          // A lost connection with nothing saved is the offline state (an empty thread, the line, messages that wait), not an error screen.
          setStatus((s) => (first && s !== "ready" ? (isNetworkError(e) ? "ready" : "error") : s));
          setOffline(true);
          await sleep(backoff);
          backoff = Math.min(30_000, backoff * 2);
        }
      }
    })();
    return () => {
      alive = false;
    };
  }, [backend, spaceId, foreground, merge, outbox]);

  // The machine took a message: its entry shows at once, before the next poll brings it.
  useEffect(() => {
    if (!outbox || !spaceId) return;
    return outbox.onDelivered((d) => {
      if (d.space !== spaceId) return;
      if (d.entry) merge([d.entry]);
      // What the message changed on the machine (an agent's queue, its status) shows at once.
      backend.members(spaceId).then(setMembers, () => {});
    });
  }, [outbox, spaceId, merge, backend]);

  const post = useCallback(
    async (text: string, opts?: { view?: ThreadView | null; replyTo?: number | null; mode?: "send" | "queue" | "steer"; agent?: string | null }) => {
      if (!spaceId || !text.trim()) return;
      const extra = { ...(opts?.view ? { view: opts.view } : {}), ...(opts?.agent ? { agent: opts.agent } : {}) };
      if (outbox) {
        await outbox.send({ space: spaceId, kind: "post", text: text.trim(), replyTo: opts?.replyTo ?? null, mode: opts?.mode ?? "send", extra });
        setReadSeq(null);
        return;
      }
      // Without an outbox (the app core is not linked here) the post is direct, and a failure is the caller's.
      const e = await backend.post(spaceId, text, { replyTo: opts?.replyTo, view: opts?.view, mode: opts?.mode, agent: opts?.agent });
      if (e) merge([e]);
      setReadSeq(null);
    },
    [backend, spaceId, merge, outbox],
  );
  const answer = useCallback(
    async (approval: number, approve: boolean, scope?: "once" | "space") => {
      if (!spaceId) return;
      if (outbox) {
        await outbox.send({ space: spaceId, kind: "answer", text: "", extra: { approval, approve, ...(scope ? { scope } : {}) } });
        return;
      }
      const e = await backend.answer(spaceId, approval, approve, { scope });
      if (e) merge([e]);
      backend.members(spaceId).then(setMembers, () => {});
    },
    [backend, spaceId, merge, outbox],
  );
  const loadEarlier = useCallback(async () => {
    const oldest = entries[0]?.seq;
    if (!spaceId || !hasMoreBefore || oldest === undefined) return;
    const p = await backend.readThread(spaceId, { before: oldest, limit: FIRST_PAGE });
    merge(p.entries);
    setHasMoreBefore(p.hasMoreBefore && p.entries.length > 0);
  }, [backend, spaceId, entries, hasMoreBefore, merge]);
  const retry = useCallback((id: string) => outbox?.retry(id), [outbox]);
  const discard = useCallback((id: string) => outbox?.discard(id), [outbox]);
  const refresh = useCallback(async () => {
    if (!spaceId) return;
    const p = await backend.readThread(spaceId, { after: cursor.current });
    merge(p.entries);
    backend.members(spaceId).then(setMembers, () => {});
  }, [backend, spaceId, merge]);

  const items = useMemo(() => foldThread(entries, { readSeq, me }), [entries, readSeq, me]);
  const view = outbox ? outbox.view(spaceId) : null;
  const pending = view?.rows ?? [];
  return {
    status,
    items,
    entries,
    members,
    live,
    me,
    role,
    error,
    offline,
    cached,
    syncedMs,
    hasMoreBefore,
    loadEarlier,
    post,
    answer,
    pending,
    outboxBanner: view?.banner ?? null,
    retry,
    discard,
    refresh,
  };
}

/**
 * The tool calls agents made in a Space, from the daemon's feed (`thread_tool_rows`), kept current with a long poll. A
 * daemon that has no such feed shows no rows (nothing is guessed); the sample backend answers in the same wire shape.
 */
export function useToolRows(spaceId: string | null): ToolGroup[] {
  const backend = useBackend();
  const foreground = useForeground();
  const [groups, setGroups] = useState<ToolGroup[]>([]);
  useEffect(() => {
    setGroups([]);
  }, [spaceId]);
  useEffect(() => {
    if (!spaceId || !foreground || !backend.toolRows) return;
    let alive = true;
    void (async () => {
      let cursor: number | null = null;
      let backoff = 2_000;
      while (alive) {
        const started = Date.now();
        try {
          const page = await backend.toolRows!(spaceId, { cursor, followMs: 15_000 });
          if (!alive) return;
          backoff = 2_000;
          cursor = page.cursor;
          setGroups((g) => mergeToolRows(g, page.groups));
          // A feed that answers at once (no long poll) is asked again after a pause.
          if (Date.now() - started < 600) await sleep(2_000);
        } catch (e) {
          if (!alive) return;
          if (isMissingThreadTool(e) || (e instanceof Error && /can't be called from another device|not available|unknown tool/i.test(e.message))) return;
          await sleep(backoff);
          backoff = Math.min(30_000, backoff * 2);
        }
      }
    })();
    return () => {
      alive = false;
    };
  }, [backend, spaceId, foreground]);
  return groups;
}
