// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * `useShareRemovalNote()`: the owner's one-line note that sharing became
 * team-only and some people can no longer open a shared computer (CUA-1883).
 * The host says who was removed (`sharing.removals`); the app core words the
 * line (`share.removalNote`); dismissing it acknowledges it
 * (`sharing.ackRemovals`), so it shows once.
 *
 * Reads the adapter directly: a host without the operation, a signed-out
 * account or a failed read shows nothing.
 */

import { useCallback, useContext, useEffect, useState } from "react";

import { BridgeContext } from "./BridgeProvider";
import type { ShareRemoval, ShareRemovalNote } from "./contracts/share";

export interface ShareRemovalNoteHook {
  /** The note while there is someone to name and it is not dismissed; null otherwise. */
  note: ShareRemovalNote | null;
  /** Closes the note and tells the host it was seen. */
  dismiss(): Promise<void>;
}

export function useShareRemovalNote(signedIn: boolean): ShareRemovalNoteHook {
  const ctx = useContext(BridgeContext);
  if (!ctx) throw new Error("bridge hooks need a <BridgeProvider> above them");
  const { adapter, core } = ctx;
  const [note, setNote] = useState<ShareRemovalNote | null>(null);

  useEffect(() => {
    if (!adapter || !signedIn) return;
    let live = true;
    void adapter.call("sharing.removals", {}).then(
      (removals: ShareRemoval[]) => {
        if (!live) return;
        setNote(core.tryCall<ShareRemovalNote | null>("share.removalNote", { removals }) ?? null);
      },
      () => undefined,
    );
    return () => {
      live = false;
    };
  }, [adapter, core, signedIn]);

  const dismiss = useCallback(async () => {
    setNote(null);
    try {
      await adapter?.call("sharing.ackRemovals", {});
    } catch {
      /* shown again next time, which is better than losing it */
    }
  }, [adapter]);

  return { note, dismiss };
}
