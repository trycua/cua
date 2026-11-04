// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Who can open "Add an agent" for a Space right now. The Space page's call
 * layout registers its own opener while it shows (and while you may add
 * agents there); the Share sheet, which opens from anywhere, offers
 * "Add an agent" only when the Space it shares has one. Pages without the
 * thread (the classic Space page, the Spaces list's menu) register none, so
 * their Share sheet stays as it was.
 */

import { create } from "zustand";

const useLaunchers = create<{ bySpace: Record<string, () => void> }>(() => ({ bySpace: {} }));

/** Registers `open` for `spaceId`; returns the unregister. */
export function registerAddAgent(spaceId: string, open: () => void): () => void {
  useLaunchers.setState((s) => ({ bySpace: { ...s.bySpace, [spaceId]: open } }));
  return () =>
    useLaunchers.setState((s) => {
      if (s.bySpace[spaceId] !== open) return s;
      const { [spaceId]: _gone, ...rest } = s.bySpace;
      return { bySpace: rest };
    });
}

/** The opener for `spaceId`, when its page offers one. */
export function useAddAgentLauncher(spaceId: string | null | undefined): (() => void) | null {
  return useLaunchers((s) => (spaceId ? (s.bySpace[spaceId] ?? null) : null));
}
