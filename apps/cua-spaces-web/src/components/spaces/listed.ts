// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useMemo } from "react";

import { sharesDesktop, useBridge, type Space } from "@/bridge";
import { useHiddenDesktops } from "@/lib/hidden-desktops";
import { countedSpaces, listedSpaces } from "@/lib/spaces";

/**
 * What the Spaces list, Home's strip and the sidebar show from the roster: your Spaces and the
 * machines' desktops that are shared, plus This Mac's desktop ("Not sharing") once something else is
 * listed. `counted` leaves that unshared desktop out (the count, "N running", the empty state);
 * `hiddenCount` is how many desktop rows "Show hidden" brings back.
 */
export function useListedSpaces(all: readonly Space[] | undefined): { listed: Space[]; counted: Space[]; hiddenCount: number } {
  const hidden = useHiddenDesktops();
  const { core } = useBridge();
  return useMemo(() => {
    const roster = all ?? [];
    // Another machine's desktop shows only while it shares it and answers.
    const shares = (s: Space) => sharesDesktop(core, s) !== false && s.sdk?.reachable !== false;
    const listed = listedSpaces(roster, hidden, shares);
    const all_ = listedSpaces(roster, new Set(), shares);
    return { listed, counted: countedSpaces(listed), hiddenCount: all_.length - listed.length };
  }, [all, hidden, core]);
}
