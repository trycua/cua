// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect, useMemo, useRef } from "react";

import { useMachines, useSpacesDefault, type SpacesDefaultHook } from "@/bridge";
import { buildRelayOnline } from "@/lib/spaces-tree";

/** How often the computers are read again while a screen that shows their status is on (the host answers from memory and asks the relay only when its copy is stale). */
export const MACHINES_REFRESH_MS = 30_000;

/**
 * The computers kept fresh, with the relay's own online answer for each (the same data as Home's fallback line).
 * The computer list counts a relay connection that hasn't been dropped yet, so a computer that powered off stays
 * "online" there for minutes; the relay's answer is what to trust. Used by the Spaces tree and the Machines page.
 * `on` false reads and refreshes nothing.
 */
export function useMachinesLive(on: boolean) {
  const { data: machines, refresh } = useMachines();
  const defaults: SpacesDefaultHook = useSpacesDefault(on);
  // Both are read again on every tick: the relay's answer is its own read, not a by-product of the list's.
  const tick = useRef(() => Promise.all([refresh(), defaults.refresh()]));
  tick.current = () => Promise.all([refresh(), defaults.refresh()]);
  useEffect(() => {
    if (!on) return;
    const t = setInterval(() => void tick.current().catch(() => {}), MACHINES_REFRESH_MS);
    return () => clearInterval(t);
  }, [on]);
  const order = defaults.resolved?.order;
  const relayOnline = useMemo(() => buildRelayOnline(order), [order]);
  return { machines, defaults, relayOnline };
}
