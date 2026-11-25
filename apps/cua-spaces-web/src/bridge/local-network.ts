// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * This Mac's Local Network access for Cua Spaces, which the macOS Spaces it
 * runs need: they live on a small network inside the Mac, and macOS blocks
 * the app from reaching them until the person allows it. Read from the Mac
 * app's `host.status` with `localNetwork` (a "local-network" permission on a
 * Mac set up to run Spaces); none on other computers, so `available` is
 * false and nothing is drawn.
 *
 * macOS has no call that asks for Local Network: it asks the first time the
 * app connects to the local network. So nothing is checked until the person
 * presses Allow, after the step explained why (`localNetworkCheck: false`).
 * Allow is `host.openSettings` with the Local Network pane, which the Mac
 * app turns into that first connect (from its background service, the
 * process that reaches the Spaces); macOS shows its prompt, and the pane
 * opens only when access stays off with no prompt waiting (said no before).
 * From then on every read checks again, so the answer shows live.
 */

import { useCallback, useContext, useEffect, useRef, useState } from "react";

import { BridgeContext } from "./BridgeProvider";

export const LOCAL_NETWORK_ID = "local-network";
export const LOCAL_NETWORK_POLL_MS = 2_500;
/** After Allow, while macOS's prompt waits for an answer. */
export const LOCAL_NETWORK_WAITING_POLL_MS = 1_000;
/** How long after Allow the row says it is waiting for the answer. */
export const LOCAL_NETWORK_WAIT_MS = 60_000;
/** When Allow was first pressed: from then on reads check, also after a relaunch. */
export const LOCAL_NETWORK_ASKED_KEY = "cua.localNetwork.askedAtMs";

function readAsked(): number | null {
  try {
    const at = Number(window.localStorage.getItem(LOCAL_NETWORK_ASKED_KEY));
    return at > 0 ? at : null;
  } catch {
    return null;
  }
}

function writeAsked(at: number): void {
  try {
    window.localStorage.setItem(LOCAL_NETWORK_ASKED_KEY, String(at));
  } catch {
    /* storage off: Allow still asks; a relaunch just shows Allow again */
  }
}

export interface LocalNetworkState {
  /** Not read yet (or there is no host to ask): `available` doesn't say anything until this is false. */
  loading: boolean;
  /** This Mac runs macOS Spaces, so it needs Local Network. False while loading and on other computers. */
  available: boolean;
  /** macOS lets Cua Spaces reach the local network. */
  granted: boolean;
  /** Allow was pressed before (this launch or an earlier one). */
  asked: boolean;
  /** Allow was pressed in the last minute and it is still off: macOS's prompt is waiting for an answer. */
  waiting: boolean;
  /** Asked, not waiting and still off: the person said no (or turned it off). */
  denied: boolean;
  /** Makes macOS ask; opens the Local Network settings when it already said no. */
  allow: () => Promise<void>;
}

/** Reads while `active`. */
export function useLocalNetwork(active: boolean): LocalNetworkState {
  const adapter = useContext(BridgeContext)?.adapter ?? null;
  const [row, setRow] = useState<{ granted: boolean; settingsUrl: string | null } | null>(null);
  const [loading, setLoading] = useState(true);
  const [askedAt, setAskedAt] = useState<number | null>(() => readAsked());
  const [now, setNow] = useState(() => Date.now());
  const live = useRef(true);
  const asked = askedAt != null;

  const refresh = useCallback(async () => {
    if (!adapter) return setLoading(false);
    const h = await adapter.call("host.status", { localNetwork: true, localNetworkCheck: asked }).catch(() => null);
    if (!live.current) return;
    setLoading(false);
    const p = h?.permissions?.find((x) => x.id === LOCAL_NETWORK_ID);
    setRow(p ? { granted: Boolean(p.granted), settingsUrl: p.settingsUrl ?? null } : null);
    setNow(Date.now());
  }, [adapter, asked]);

  const granted = Boolean(row?.granted);
  const waiting = asked && !granted && now - (askedAt ?? 0) < LOCAL_NETWORK_WAIT_MS;

  useEffect(() => {
    live.current = true;
    if (!active) return;
    void refresh();
    const t = setInterval(() => void refresh(), waiting ? LOCAL_NETWORK_WAITING_POLL_MS : LOCAL_NETWORK_POLL_MS);
    const again = () => {
      if (document.visibilityState !== "hidden") void refresh();
    };
    window.addEventListener("focus", again);
    document.addEventListener("visibilitychange", again);
    return () => {
      live.current = false;
      clearInterval(t);
      window.removeEventListener("focus", again);
      document.removeEventListener("visibilitychange", again);
    };
  }, [active, refresh, waiting]);

  const allow = useCallback(async () => {
    const url = row?.settingsUrl;
    if (!adapter || !url) return;
    const at = Date.now();
    writeAsked(at);
    setAskedAt(at);
    setNow(at);
    await adapter.call("host.openSettings", { url });
  }, [adapter, row]);

  return { loading, available: row != null, granted, asked, waiting, denied: asked && !granted && !waiting, allow };
}
