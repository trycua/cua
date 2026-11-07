// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect, useRef } from "react";

import { useSpaces } from "@/bridge";
import { useChildren } from "@/components/children/use-children";
import { reusableHome } from "@/lib/agent-home/home-space";

/**
 * Keeps the agent's home warm while Home is open: a home that was turned off
 * (suspended or stopped) is turned back on in the background, so the first
 * request after opening the app finds a running Space and does not wait for a
 * boot. Once per Space per page load; a home that is not there is not made
 * (that is the first request's job), and nothing is stopped by this: a home
 * stays on until the person turns it off.
 */
export function useKeepHomeWarm(agent: string, enabled: boolean): void {
  const { data: spaces, startSpace } = useSpaces();
  const children = useChildren();
  const asked = useRef(new Set<string>());
  const labelled = children?.homesOf(agent);
  useEffect(() => {
    if (!enabled) return;
    const id = reusableHome(agent, spaces, false, labelled);
    const row = id ? spaces?.find((s) => s.id === id) : undefined;
    if (!id || !row || row.status !== "suspended" || asked.current.has(id)) return;
    asked.current.add(id);
    Promise.resolve(startSpace(id)).catch(() => asked.current.delete(id));
  }, [agent, enabled, spaces, labelled, startSpace]);
}
