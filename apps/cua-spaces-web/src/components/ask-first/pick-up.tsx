// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect, useState } from "react";

import { isUnsupported, useBridge, type RecentThread } from "@/bridge";

/** How many chats the Ready screen offers. */
export const PICK_UP_LIMIT = 4;

/**
 * The recent Claude Code, Codex and Hermes chats on this computer
 * (`agents.recentThreads`), newest first. Null until the host answered;
 * empty when there are none or the host can't list them.
 */
export function useRecentThreads(limit = PICK_UP_LIMIT): RecentThread[] | null {
  const { data } = useBridge();
  const [threads, setThreads] = useState<RecentThread[] | null>(null);
  useEffect(() => {
    let live = true;
    if (!data) return;
    data
      .call("agents.recentThreads", { limit })
      .then((rows) => live && setThreads(rows))
      .catch((e: unknown) => {
        // A host or daemon without the list, or a store it couldn't read: the section stays hidden.
        if (live) setThreads([]);
        if (!isUnsupported(e)) console.warn("agents.recentThreads failed", e);
      });
    return () => {
      live = false;
    };
  }, [data, limit]);
  return threads;
}

/** "25 min ago", "3 h ago", "2 days ago". */
export function agoLabel(ms: number, now = Date.now()): string {
  const s = Math.max(0, Math.round((now - ms) / 1000));
  if (s < 60) return "just now";
  const m = Math.round(s / 60);
  if (m < 60) return `${m} min ago`;
  const h = Math.round(m / 60);
  if (h < 24) return `${h} h ago`;
  const d = Math.round(h / 24);
  return d === 1 ? "yesterday" : `${d} days ago`;
}
