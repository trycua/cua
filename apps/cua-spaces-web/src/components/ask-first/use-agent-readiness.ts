// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect, useState } from "react";

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
export function readinessOf(member: Pick<AgentMember, "status"> | undefined, view: AgentsView | null, selected: AgentSelection | null): AgentReadiness {
  if (!selected) return view?.groups.some((g) => g.state === "needs-reconnect") ? "reconnect" : "none";
  if (!member) return "none";
  if (member.status === "failed") return "reconnect";
  return member.status === "idle" || member.status === "working" || member.status === "waiting" ? "ready" : "starting";
}

export function useAgentReadiness(agent: string, enabled: boolean, view: AgentsView | null, selected: AgentSelection | null): AgentReadiness {
  const { data } = useBridge();
  const { data: spaces } = useSpaces();
  const children = useChildren();
  const home = enabled ? reusableHome(agent, spaces, false, children?.homesOf(agent)) : null;
  const [member, setMember] = useState<Pick<AgentMember, "status"> | undefined>(undefined);
  useEffect(() => {
    setMember(undefined);
    if (!home || !data || !threadOpAvailable(data.mode, "thread.members")) return;
    let live = true;
    const read = () =>
      void data
        .call("thread.members", { spaceId: home })
        .then((r) => {
          if (live) setMember(r.members.find((m) => m.harness === agent));
        })
        .catch(() => {});
    read();
    const t = setInterval(read, READINESS_POLL_MS);
    return () => {
      live = false;
      clearInterval(t);
    };
  }, [home, data, agent]);
  return readinessOf(member, view, selected);
}
