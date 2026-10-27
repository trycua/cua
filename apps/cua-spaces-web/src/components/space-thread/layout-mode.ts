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
