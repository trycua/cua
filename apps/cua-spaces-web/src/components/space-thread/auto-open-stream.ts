// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect, useRef } from "react";

/** A stream in the Space's Stream list that is not its desktop (a window, from this Space or from one under it). */
export interface StreamEntry {
  id: string;
}

/**
 * The first stream that appeared after the page opened, or null. `baseline` is what the list held when it
 * first loaded: a stream that was already there is not new, and neither is one the page has seen.
 */
export function firstNewStream<T extends StreamEntry>(baseline: ReadonlySet<string>, windows: readonly T[]): T | null {
  return windows.find((w) => !baseline.has(w.id)) ?? null;
}

/**
 * A home Space keeps its desktop folded, but the first window stream that appears in its Stream list while
 * the page is open (an app an agent opened, here or in a Space under it) opens by itself, so the person sees
 * what the agent just opened. Once per Space per page session: later streams only join the list. Never
 * over a choice the person made for this Space (`chosen`), and not for a stream that was already listed when
 * the list loaded. It reads only the list, so where a stream comes from does not matter.
 */
export function useAutoOpenFirstStream<T extends StreamEntry>({
  spaceId,
  enabled,
  ready,
  windows,
  chosen,
  onOpen,
}: {
  spaceId: string;
  /** The Space is a home. */
  enabled: boolean;
  /** The Stream list has loaded (an empty list before that is not "no streams"). */
  ready: boolean;
  windows: readonly T[];
  /** The person already chose to show or fold this Space's stream. */
  chosen: () => boolean;
  onOpen: (row: T) => void;
}): void {
  const state = useRef<{ spaceId: string; baseline: Set<string> | null; done: boolean }>({ spaceId, baseline: null, done: false });
  if (state.current.spaceId !== spaceId) state.current = { spaceId, baseline: null, done: false };
  useEffect(() => {
    const s = state.current;
    if (!ready) return;
    if (s.baseline === null) {
      s.baseline = new Set(windows.map((w) => w.id));
      return;
    }
    if (s.done) return;
    const fresh = firstNewStream(s.baseline, windows);
    if (!fresh) return;
    s.done = true;
    if (enabled && !chosen()) onOpen(fresh);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- reads the latest list and callbacks on each list change
  }, [ready, windows, enabled, spaceId]);
}
