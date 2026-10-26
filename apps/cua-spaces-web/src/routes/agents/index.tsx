// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useEffect, useState } from "react";

import { useAgentChats, useFeatureRecorded, useSession } from "@/bridge";
import { AgentDmView, AgentList, AgentPage, AgentsEmpty, NewAgentSheet } from "@/components/agents/agent-chat";
import type { AddAgentStage } from "@/components/add-agent/add-agent-sheet";
import { useFlag } from "@/bridge/flags";
import { LegacyAgentsPage } from "@/legacy/routes/agents/index";

/** Which agent's DM is open, and in which of its Spaces (none: all of them). */
interface ChatSearch {
  agent?: string;
  space?: string;
  run?: string;
}

export const Route = createFileRoute("/agents/")({
  component: AgentsChatPageSwitch,
  validateSearch: (s: Record<string, unknown>): ChatSearch => ({
    agent: typeof s.agent === "string" ? s.agent : undefined,
    space: typeof s.space === "string" ? s.space : undefined,
    // The page before the redesign opens a run with `?run=`.
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
  const search = Route.useSearch();
  const navigate = useNavigate({ from: "/agents/" });
  const { data: session } = useSession();
  const identity = session?.signedIn ? (session.account.identity ?? null) : null;
  const { chats, spaces, me, refresh } = useAgentChats();
  const now = useNow();
  const [sheet, setSheet] = useState<AddAgentStage | null>(null);
  const [info, setInfo] = useState(false);
  const chat = chats?.find((c) => c.key === search.agent) ?? (search.agent ? null : (chats?.[0] ?? null));
  const space = chat && search.space && chat.spaces.some((s) => s.spaceId === search.space) ? search.space : null;
  const select = (agent: string, sp?: string) => void navigate({ search: { agent, space: sp }, replace: true });
  const taken = (spaceId: string) => (chats ?? []).flatMap((c) => c.spaces).filter((s) => s.spaceId === spaceId).map((s) => s.handle);
  return (
    <div data-agents-chat="" className="flex h-full min-h-0">
      <AgentList chats={chats ?? []} me={me} selected={chat?.key ?? null} onSelect={select} onNew={() => setSheet("pick")} now={now} />
      {chats === null ? (
        <div className="flex-1" />
      ) : chat ? (
        <>
          <AgentDmView key={chat.key} chat={chat} space={space} onSpace={(sp) => select(chat.key, sp ?? undefined)} identity={identity} onInfo={() => setInfo(true)} me={me} />
          <AgentPage chat={chat} open={info} onOpenChange={setInfo} me={me} identity={identity} />
        </>
      ) : chats.length === 0 ? (
        <AgentsEmpty onStart={() => setSheet("start")} onConnect={() => setSheet("connect")} />
      ) : (
        <div className="flex flex-1 items-center justify-center text-[13px] text-muted-foreground">That agent isn't in any of your Spaces now.</div>
      )}
      <NewAgentSheet
        open={sheet !== null}
        stage={sheet ?? undefined}
        onOpenChange={(o) => {
          if (!o) {
            setSheet(null);
            refresh();
          }
        }}
        spaces={spaces.map((s) => ({ id: s.id, name: s.name }))}
        taken={taken}
      />
    </div>
  );
}

/** The redesign behind `spaces_redesign` (Settings → Advanced → Labs); off, the page as it was. */
function AgentsChatPageSwitch() {
  return useFlag("spaces_redesign") ? <AgentsChatPage /> : <LegacyAgentsPage />;
}
