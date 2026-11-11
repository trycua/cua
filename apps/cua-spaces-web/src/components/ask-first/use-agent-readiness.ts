// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect, useRef, useState } from "react";

import { threadOpAvailable, useBridge, useSpaces, type AgentMember, type AgentsView } from "@/bridge";
import { useChildren } from "@/components/children/use-children";
import { reusableHome } from "@/lib/agent-home/home-space";
import type { AgentSelection } from "@/lib/ask-first-model";

import type { AgentReadiness } from "./agent-status";

/** How often the member's state is read while Home is open. */
export const READINESS_POLL_MS = 2_500;

/**
 * Whether the agent behind the box is ready: its home has the member for the
 * app the model runs on, kept on standby with its session open (`idle`),
 * still starting (`asleep` or `working` before it has been ready), or a
 * provider's sign-in ran out and nothing connected is left to pick.
 */
/** How long an agent may read "Starting agent" while its session is closed (`asleep`, `paused`). Past it the session is
 * not on its way up: it is closed after a run and wakes with the next message, so the box says it is ready. */
export const STARTING_MAX_MS = 45_000;

export function readinessOf(
  member: Pick<AgentMember, "status"> | undefined,
  view: AgentsView | null,
  selected: AgentSelection | null,
  waitedMs = 0,
): AgentReadiness {
  if (!selected) return view?.groups.some((g) => g.state === "needs-reconnect") ? "reconnect" : "none";
  if (!member) return "none";
  if (member.status === "failed") return "reconnect";
  if (member.status === "idle" || member.status === "working" || member.status === "waiting") return "ready";
  return waitedMs >= STARTING_MAX_MS ? "ready" : "starting";
}

export function useAgentReadiness(agent: string, enabled: boolean, view: AgentsView | null, selected: AgentSelection | null): AgentReadiness {
  const { data } = useBridge();
  const { data: spaces } = useSpaces();
  const children = useChildren();
  const home = enabled ? reusableHome(agent, spaces, false, children?.homesOf(agent)) : null;
  const [member, setMember] = useState<Pick<AgentMember, "status"> | undefined>(undefined);
  // When this home's agent was first seen not ready (a run that ended leaves its session closed, not starting).
  const notReadySince = useRef<number | null>(null);
  useEffect(() => {
    setMember(undefined);
    notReadySince.current = null;
    if (!home || !data || !threadOpAvailable(data.mode, "thread.members")) return;
    let live = true;
    const read = () =>
      void data
        .call("thread.members", { spaceId: home })
        .then((r) => {
          if (!live) return;
          const found = r.members.find((m) => m.harness === agent);
          const up = found?.status === "idle" || found?.status === "working" || found?.status === "waiting";
          if (up || !found) notReadySince.current = null;
          else notReadySince.current ??= Date.now();
          setMember(found);
        })
        .catch(() => {});
    read();
    const t = setInterval(read, READINESS_POLL_MS);
    return () => {
      live = false;
      clearInterval(t);
    };
  }, [home, data, agent]);
  return readinessOf(member, view, selected, notReadySince.current === null ? 0 : Date.now() - notReadySince.current);
}
