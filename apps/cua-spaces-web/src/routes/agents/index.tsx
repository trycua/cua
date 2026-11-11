// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useEffect, useState } from "react";

import { chatKey, useAgentChats, useFeatureRecorded, type AgentChat, useKeysRecheck, useSession } from "@/bridge";
import { AgentDmView, AgentList, AgentPage, AgentsUnset } from "@/components/agents/agent-chat";
import { useAgentAttempts, useAttemptAction, useAttemptRemoval } from "@/hooks/use-agent-attempts";

/** Which agent's DM is open (each agent in each Space is its own entry), or `new`: none, the box to start one. */
interface ChatSearch {
  agent?: string;
  new?: boolean;
  run?: string;
}

export const Route = createFileRoute("/agents/")({
  component: AgentsChatPage,
  validateSearch: (s: Record<string, unknown>): ChatSearch => ({
    agent: typeof s.agent === "string" ? s.agent : undefined,
    new: s.new === true || s.new === "1" || s.new === "true" ? true : undefined,
    // The older Agents page opened a run with `?run=`.
    run: typeof s.run === "string" ? s.run : undefined,
  }),
});

/** Re-renders every `ms` so ages stay current. */
function useNow(ms = 30_000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), ms);
    return () => clearInterval(t);
  }, [ms]);
  return now;
}

/**
 * The Agents tab (CUA-2266): every agent you can message, Needs you pinned
 * on top, and the DM with the one you pick. The persistent agents' memory,
 * routines and runs are one click away ("Show all").
 */
function AgentsChatPage() {
  useFeatureRecorded("agents_page_open");
  // Opening the Agents page asks the providers whether the saved sign-ins and keys still work.
  useKeysRecheck();
  const search = Route.useSearch();
  const navigate = useNavigate({ from: "/agents/" });
  const { data: session } = useSession();
  const identity = session?.signedIn ? (session.account.identity ?? null) : null;
  // The one list (agents in your Spaces and this device's runs that haven't started): Home shows it too.
  const attempts = useAgentAttempts();
  const act = useAttemptAction();
  const removal = useAttemptRemoval();
  const { chats, me, loading } = useAgentChats(attempts);
  const now = useNow();
  const [info, setInfo] = useState(false);
  // A run that hasn't started opens its Space's thread; only agents have a DM here.
  // New agent clears the selection: nothing is open and the box to start one shows.
  // `agent` is an entry's key; a link by who the agent is (owner, kind and name) opens its first entry.
  const byKey = (c: AgentChat) =>
    c.kind === "agent" && (c.key === search.agent || (c.spaces[0] && chatKey(c.spaces[0].spaceId, c.spaces[0].member) === search.agent));
  const chat = search.new ? null : (chats?.find(byKey) ?? (search.agent ? null : (chats?.find((c) => c.kind === "agent") ?? null)));
  const select = (agent: string) => void navigate({ search: { agent }, replace: true });
  return (
    <div data-agents-chat="" className="flex h-full min-h-0">
      <AgentList
        chats={chats}
        me={me}
        selected={chat?.key ?? null}
        onSelect={select}
        onOpenRun={(c) => c.places[0] && void navigate({ to: "/spaces/$spaceId", params: { spaceId: c.places[0].spaceId } })}
        onAction={act}
        onNew={() => void navigate({ search: { new: true }, replace: true })}
        onRemove={removal.remove}
        onClearRuns={() => removal.clear((chats ?? []).filter((c) => c.kind === "attempt"))}
        now={now}
        loading={loading}
      />
      {chats === null || (!search.new && loading > 0 && chats.every((c) => c.kind !== "agent")) ? (
        <div className="flex-1" />
      ) : chat ? (
        <>
          <AgentDmView key={chat.key} chat={chat} identity={identity} onInfo={() => setInfo(true)} me={me} />
          <AgentPage chat={chat} open={info} onOpenChange={setInfo} me={me} identity={identity} />
        </>
      ) : search.new || chats.every((c) => c.kind !== "agent") ? (
        <AgentsUnset />
      ) : (
        <div className="flex flex-1 items-center justify-center text-[13px] text-muted-foreground">That agent isn't in any of your Spaces now.</div>
      )}
    </div>
  );
}
