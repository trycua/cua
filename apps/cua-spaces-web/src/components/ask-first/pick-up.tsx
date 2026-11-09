// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect, useState } from "react";

import { t } from "@/i18n";
import { isUnsupported, onlyAgents, threadAgentOf, useBridge, type DataAdapter, type RecentThread } from "@/bridge";

/** How many chats the Ready screen offers. */
export const PICK_UP_LIMIT = 4;

/** How long a listing is reused (the daemon keeps its own for as long). */
const KEEP_MS = 30_000;

/** Listings asked for, per host and agents: the Ready screen reuses the one started when the agent was picked. */
const asked = new WeakMap<DataAdapter, Map<string, { at: number; rows: Promise<RecentThread[]>; value?: RecentThread[] }>>();

/** The agents whose chats can be read, in one order (`codex` and `openai-codex` are one agent). */
function readable(agents: readonly string[] | undefined): string[] | undefined {
  if (!agents) return undefined;
  return [...new Set(agents.map(threadAgentOf).filter((a) => a !== null))].sort();
}

/**
 * The recent chats of `agents` on this computer (`agents.recentThreads`, read by the daemon), newest first, at most
 * `limit`. Started once per host and agents and reused for a little while, so asking early (when the agent is
 * picked) makes the Ready screen's list show at once. Empty when none of `agents` keeps chats Cua can read, or the
 * host can't list them. Only titles, folders and times, in this page's memory.
 */
export function recentThreads(data: DataAdapter, agents?: readonly string[], limit = PICK_UP_LIMIT): Promise<RecentThread[]> {
  const want = readable(agents);
  if (want && want.length === 0) return Promise.resolve([]);
  const key = `${want?.join(",") ?? "*"}:${limit}`;
  let byKey = asked.get(data);
  if (!byKey) asked.set(data, (byKey = new Map()));
  const known = byKey.get(key);
  if (known && Date.now() - known.at < KEEP_MS) return known.rows;
  const entry: { at: number; rows: Promise<RecentThread[]>; value?: RecentThread[] } = { at: Date.now(), rows: Promise.resolve([]) };
  entry.rows = data
    .call("agents.recentThreads", want ? { limit, agents: want } : { limit })
    // A host that answers with more, or with other agents' chats (an older daemon), is cut here.
    .then((r) => (entry.value = onlyAgents(r, want).slice(0, limit)))
    .catch((e: unknown) => {
      byKey.delete(key);
      // A host or daemon without the list, or a store it couldn't read: the section stays hidden.
      if (!isUnsupported(e)) console.warn("agents.recentThreads failed", e);
      return [];
    });
  byKey.set(key, entry);
  return entry.rows;
}

/** The chats {@link recentThreads} already has (no ask): `[]` when none of `agents` keeps chats Cua reads. */
export function knownRecentThreads(data: DataAdapter | null, agents?: readonly string[], limit = PICK_UP_LIMIT): RecentThread[] | null {
  const want = readable(agents);
  if (want && want.length === 0) return [];
  const known = data ? asked.get(data)?.get(`${want?.join(",") ?? "*"}:${limit}`) : undefined;
  return known && Date.now() - known.at < KEEP_MS ? (known.value ?? null) : null;
}

/**
 * {@link recentThreads} as a hook: null until the host answered (or straight away, when an earlier ask already
 * answered), then the chats.
 */
export function useRecentThreads(limit = PICK_UP_LIMIT, agents?: readonly string[]): RecentThread[] | null {
  const { data } = useBridge();
  const key = agents ? [...agents].sort().join(",") : "*";
  const list = key === "*" ? undefined : key.split(",").filter(Boolean);
  const [threads, setThreads] = useState<RecentThread[] | null>(() => knownRecentThreads(data, list, limit));
  useEffect(() => {
    let live = true;
    if (!data) return;
    const want = key === "*" ? undefined : key.split(",").filter(Boolean);
    setThreads(knownRecentThreads(data, want, limit));
    void recentThreads(data, want, limit).then((rows) => live && setThreads(rows));
    return () => {
      live = false;
    };
  }, [data, limit, key]);
  return threads;
}

/** "25 min ago", "3 h ago", "2 days ago". */
export function agoLabel(ms: number, now = Date.now()): string {
  const s = Math.max(0, Math.round((now - ms) / 1000));
  if (s < 60) return t("home.pick_up.ago_now");
  const m = Math.round(s / 60);
  if (m < 60) return t("home.pick_up.ago_minutes", { count: m });
  const h = Math.round(m / 60);
  if (h < 24) return t("home.pick_up.ago_hours", { count: h });
  const d = Math.round(h / 24);
  return d === 1 ? t("home.pick_up.ago_yesterday") : t("home.pick_up.ago_days", { count: d });
}
