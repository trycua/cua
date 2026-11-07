// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Cua Spaces' own Screen Recording and Accessibility (`driver.permissions`,
 * `ops/driver.ts`), the grants Cua Driver uses. One read for every place that
 * shows them: the first run's permissions step, its next steps, the Cua
 * Driver page and This Mac. Each reads while it shows: at once, every few
 * seconds (every second after Allow) and when the app comes back to the
 * front (System Settings is where the person turns them on), so a grant
 * shows without a click, also after the app reopens for Screen Recording.
 *
 * Allow is `host.openSettings` with the row's pane, which the Mac app turns
 * into its permission guide (PermissionGuide). The host restarts a driver
 * that still says no once both are on.
 *
 * A host with no permissions to give (Windows, Linux, an older app) answers
 * null or refuses: `available` is false and nothing is drawn.
 */

import { useCallback, useContext, useEffect, useRef, useState } from "react";

import { BridgeContext } from "./BridgeProvider";
import type { AppPermissions, DriverPermission } from "./ops/driver";

export type { AppPermissions } from "./ops/driver";

export const APP_PERMISSIONS_POLL_MS = 2_500;
/** While the person is in System Settings after Allow: every second, as Cua Driver's own guide checks. */
export const APP_PERMISSIONS_WAITING_POLL_MS = 1_000;
/** How long after Allow the rows say they are waiting (the guide in the Mac app gives up after as long). */
export const APP_PERMISSIONS_WAIT_MS = 15 * 60_000;
/**
 * When Allow was last pressed, kept across a relaunch: Screen Recording only applies after the app reopens, and the
 * rows must still say they are checking when it comes back.
 */
export const APP_PERMISSIONS_ASKED_KEY = "cua.appPermissions.askedAtMs";

export function readAskedAt(now = Date.now()): number | null {
  try {
    const at = Number(window.localStorage.getItem(APP_PERMISSIONS_ASKED_KEY));
    return at > 0 && now - at >= 0 && now - at < APP_PERMISSIONS_WAIT_MS ? at : null;
  } catch {
    return null;
  }
}

function writeAskedAt(at: number | null): void {
  try {
    if (at == null) window.localStorage.removeItem(APP_PERMISSIONS_ASKED_KEY);
    else window.localStorage.setItem(APP_PERMISSIONS_ASKED_KEY, String(at));
  } catch {
    /* storage off: the rows still read live, they just don't say they are waiting after a relaunch */
  }
}

export interface AppPermissionsState {
  /** Null until the first answer, and on a host with none to give. */
  data: AppPermissions | null;
  /** The first read hasn't answered. */
  loading: boolean;
  /** The host has the two permissions (a Mac). False while loading. */
  available: boolean;
  /** Both are on. */
  allGranted: boolean;
  /** Allow was pressed in the last 15 minutes (this launch or the one before) and one is still off: the rows say they are checking. */
  waiting: boolean;
  /** Opens the app's guide to the row's pane in System Settings. */
  allow: (id: DriverPermission["id"]) => Promise<void>;
  /** Reads them again now. */
  refresh: () => Promise<void>;
}

/** Reads while `active` (default on). */
export function useAppPermissions(active = true): AppPermissionsState {
  const adapter = useContext(BridgeContext)?.adapter ?? null;
  const [data, setData] = useState<AppPermissions | null>(null);
  const [loading, setLoading] = useState(true);
  const reading = useRef(false);
  const live = useRef(true);
  const [askedAt, setAskedAt] = useState<number | null>(() => readAskedAt());

  const refresh = useCallback(async () => {
    if (!adapter) return setLoading(false);
    if (reading.current) return;
    reading.current = true;
    try {
      const next = await adapter.call("driver.permissions", {}).catch(() => null);
      if (live.current) setData(next);
    } finally {
      reading.current = false;
      if (live.current) setLoading(false);
    }
  }, [adapter]);

  const permissions = data?.permissions ?? [];
  const allGranted = permissions.length > 0 && permissions.every((p) => p.granted);
  const waiting = askedAt != null && permissions.length > 0 && !allGranted;

  // Both on: nothing left to wait for, here or after the next launch.
  useEffect(() => {
    if (allGranted && askedAt != null) {
      writeAskedAt(null);
      setAskedAt(null);
    }
  }, [allGranted, askedAt]);

  useEffect(() => {
    live.current = true;
    if (!active) return;
    void refresh();
    const t = setInterval(() => void refresh(), waiting ? APP_PERMISSIONS_WAITING_POLL_MS : APP_PERMISSIONS_POLL_MS);
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

  const allow = useCallback(
    async (id: DriverPermission["id"]) => {
      const url = data?.permissions.find((p) => p.id === id)?.settingsUrl;
      if (!adapter || !url) return;
      const now = Date.now();
      writeAskedAt(now);
      setAskedAt(now);
      await adapter.call("host.openSettings", { url });
      void refresh();
    },
    [adapter, data, refresh],
  );

  return {
    data,
    loading,
    available: permissions.length > 0,
    allGranted,
    waiting,
    allow,
    refresh,
  };
}
