// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The Spaces and machine desktops pinned to the notch ("Pin to notch" in a
 * Space's "⋯" menu). The notch lists them first.
 *
 * The state is the core's `AppSettings.notchPinned` (ids in the order they
 * were pinned), kept by the host with the rest of the app's settings
 * (`settings.get` answers it in `values.notchPinned`; `settings.set` with key
 * `notchPinned` saves it), so the notch, the SwiftUI app and the Electron
 * app agree and a pin survives a restart. Pinning changes nothing about
 * access: it only says which tiles the notch keeps first.
 *
 * A host that keeps no core settings refuses the save: the page then keeps
 * the pins for this session.
 */

import { useCallback, useMemo, useState } from "react";

import { useBridge, useSettings } from "./hooks";

const NONE: readonly string[] = [];

export interface NotchPins {
  /** The pinned ids, in pin order. */
  ids: readonly string[];
  /** The same, to ask `has(id)`. */
  pinned: ReadonlySet<string>;
  /** The app core can pin (it answers `notch.togglePin`). */
  canPin: boolean;
  /** Pins `id`, or unpins it when it is pinned. */
  toggle(id: string): Promise<void>;
}

/** The pins now, and what changes them. */
export function useNotchPins(): NotchPins {
  const settings = useSettings();
  const { core } = useBridge();
  // Set only when the host refused to keep them.
  const [session, setSession] = useState<readonly string[] | null>(null);
  const ids = session ?? settings.data?.values.notchPinned ?? NONE;
  const pinned = useMemo(() => new Set(ids), [ids]);
  const canPin = core.status === "ready";
  const toggle = useCallback(
    async (id: string) => {
      const next = core.tryCall<string[]>("notch.togglePin", { pinned: ids, id });
      if (!next) return;
      if (session) return void setSession(next);
      try {
        await settings.updateSetting("notchPinned", next);
      } catch {
        // The host keeps no core settings: this window remembers.
        setSession(next);
      }
    },
    [core, ids, session, settings],
  );
  return { ids, pinned, canPin, toggle };
}
