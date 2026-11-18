// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useCallback, useState } from "react";

/**
 * How a Space's page is laid out:
 * - `call`: the desktop is the main area, the thread under it (the call storyboard);
 * - `thread`: the thread is the main area and the desktop shrinks into a
 *   floating mini-player (`MiniPlayer`).
 *
 * Remembered per Space on this device.
 */
export type SpaceLayout = "call" | "thread";

const key = (spaceId: string) => `cua-spaces:layout:${spaceId}`;

function read(k: string): string | null {
  try {
    return globalThis.localStorage?.getItem(k) ?? null;
  } catch {
    return null;
  }
}

function write(k: string, v: string) {
  try {
    globalThis.localStorage?.setItem(k, v);
  } catch {
    // Blocked storage: the choice just isn't remembered.
  }
}

export function savedLayout(spaceId: string): SpaceLayout {
  return read(key(spaceId)) === "thread" ? "thread" : "call";
}

/** The Space's layout and its setter (remembered per Space). */
export function useSpaceLayout(spaceId: string): [SpaceLayout, (next: SpaceLayout) => void] {
  const [state, setState] = useState<{ spaceId: string; layout: SpaceLayout }>(() => ({ spaceId, layout: savedLayout(spaceId) }));
  const layout = state.spaceId === spaceId ? state.layout : savedLayout(spaceId);
  const set = useCallback(
    (next: SpaceLayout) => {
      write(key(spaceId), next);
      setState({ spaceId, layout: next });
    },
    [spaceId],
  );
  return [layout, set];
}

/**
 * Whether the Space's stream is hidden (only the thread shows): a click on
 * the already-selected stream in the sidebar hides it, another shows it
 * again. Remembered per Space on this device; while hidden the stream is not
 * drawn at all, so its connection is released. `hiddenByDefault` is what a
 * Space nobody chose for yet does (an agent's home starts hidden: the chat
 * is the page, the desktop opens on demand); a choice, either way, is kept.
 */
const hiddenKey = (spaceId: string) => `cua-spaces:stream-hidden:${spaceId}`;

export const savedStreamHidden = (spaceId: string, hiddenByDefault = false): boolean => {
  const v = read(hiddenKey(spaceId));
  return v === null ? hiddenByDefault : v === "1";
};

/** The person has shown or folded this Space's stream (a default is not a choice). */
export const hasStreamChoice = (spaceId: string): boolean => read(hiddenKey(spaceId)) !== null;

export function useStreamHidden(spaceId: string, hiddenByDefault = false): [boolean, (hidden: boolean) => void, () => void] {
  const [state, setState] = useState<{ spaceId: string; hidden: boolean | null }>(() => ({ spaceId, hidden: null }));
  // Until the person chooses, the default decides (it may change once the Space is known to be a home).
  const hidden = state.spaceId === spaceId && state.hidden !== null ? state.hidden : savedStreamHidden(spaceId, hiddenByDefault);
  const set = useCallback(
    (next: boolean) => {
      write(hiddenKey(spaceId), next ? "1" : "0");
      setState({ spaceId, hidden: next });
    },
    [spaceId],
  );
  // Shows the stream for this visit only: the Space's saved choice (or its default, folded for a home) stands next time,
  // so an app an agent opens does not turn a home's folded desktop into a choice the person never made.
  const showNow = useCallback(() => setState({ spaceId, hidden: false }), [spaceId]);
  return [hidden, set, showNow];
}
