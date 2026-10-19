// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * `useLinkNotice()`: the one-time notice for machines that saw the older
 * "anonymous" usage-data wording (CUA-1881). Usage data is linked to the
 * signed-in account through a random ID, and nothing links before the person
 * closes this notice. The host says whether it is due and owns the words
 * (`telemetry.linkNotice`); both buttons close it
 * (`telemetry.acknowledgeLinkNotice`), and "Turn off usage data" turns the
 * switch off first, through the same path as Settings.
 *
 * Reads the adapter directly, so a host without the operation, or a failed
 * read, shows nothing (and the host links nothing either).
 */

import { useCallback, useContext, useEffect, useState } from "react";
import { BridgeContext } from "./BridgeProvider";
import { useSettings } from "./hooks";
import type { TelemetryLinkNotice } from "./ops/telemetry";

export interface LinkNoticeHook {
  /** The notice while it is due and not closed yet; null otherwise. */
  notice: TelemetryLinkNotice | null;
  /** A button is working (the notice stays up until it finishes). */
  busy: boolean;
  /** "Turn off usage data" could not turn it off: the notice stays up. */
  error: string | null;
  /** "Got it": usage data stays on. */
  keep(): Promise<void>;
  /** "Turn off usage data": turns it off, then closes. */
  turnOff(): Promise<void>;
}

export const TURN_OFF_FAILED = "Couldn't turn off usage data. Try again, or change it in Settings, Privacy.";

export function useLinkNotice(): LinkNoticeHook {
  const ctx = useContext(BridgeContext);
  if (!ctx) throw new Error("bridge hooks need a <BridgeProvider> above them");
  const { adapter } = ctx;
  const { updateSetting } = useSettings();
  const [notice, setNotice] = useState<TelemetryLinkNotice | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!adapter) return;
    let live = true;
    void adapter.call("telemetry.linkNotice", {}).then(
      (n) => live && setNotice(n?.due ? n : null),
      () => undefined,
    );
    return () => {
      live = false;
    };
  }, [adapter]);

  // Closing never fails the person: a host that could not record it asks again next launch.
  const close = useCallback(async () => {
    try {
      await adapter?.call("telemetry.acknowledgeLinkNotice", {});
    } catch {
      /* asked again at the next launch */
    }
    setNotice(null);
  }, [adapter]);

  const keep = useCallback(async () => {
    setBusy(true);
    try {
      await close();
    } finally {
      setBusy(false);
    }
  }, [close]);

  const turnOff = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      await updateSetting("telemetry", false);
    } catch {
      // Still on: closing now would link what the person just said no to.
      setError(TURN_OFF_FAILED);
      setBusy(false);
      return;
    }
    try {
      await close();
    } finally {
      setBusy(false);
    }
  }, [updateSetting, close]);

  return { notice, busy, error, keep, turnOff };
}
