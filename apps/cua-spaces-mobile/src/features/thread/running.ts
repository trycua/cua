// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/** "12s", "1m 05s": how long something that began at `since` has been running at `now` (both in ms). */
export function runningFor(since: number, now: number): string {
  const s = Math.max(0, Math.floor((now - since) / 1000));
  return s < 60 ? `${s}s` : `${Math.floor(s / 60)}m ${String(s % 60).padStart(2, "0")}s`;
}

/** The page a child row opens: the Space once it exists, else its being-made row (what the host lists while it is made). */
export function childPageId(space: { space: string; key?: string; state?: string }): string {
  if (space.state === "ready" || !space.key) return space.space;
  return `pending:agent-${space.key.replace(/:(\d+)$/, "-$1")}`;
}
