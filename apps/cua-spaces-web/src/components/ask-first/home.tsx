// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect, useMemo, useRef, useState } from "react";

import { useAgentChats, useSpaces, type AgentRunSummary } from "@/bridge";
import { useAgentAttempts } from "@/hooks/use-agent-attempts";
import { OsIcon } from "@/components/os-icon";
import { SUGGESTIONS, useAskTasks, withoutStaleTasks, type AskTask, type TaskState } from "@/lib/ask-first";
import { useListedSpaces } from "@/components/spaces/listed";
import { cn } from "@/lib/utils";

import { AskBox } from "./ask-box";
import { useAskComposer } from "./use-ask-composer";
import { BringTheRest } from "./bring-the-rest";
import { AgentStatus } from "./agent-status";
import { useAgentReadiness } from "./use-agent-readiness";
import { useKeepHomeWarm } from "./use-keep-home-warm";
import { YourAgents, YourSpaces as YourSpacesRow } from "./home-sections";
import { PrepareLine } from "@/components/prepare/prepare-line";
import { usePrepareAction } from "@/components/prepare/use-prepare-action";
import { usePrepare } from "@/lib/prepare/store";

/** The run a task follows, from the runs in your running Spaces. */
export function taskRun(runs: readonly AgentRunSummary[] | undefined, task: AskTask): AgentRunSummary | undefined {
  return task.runId ? runs?.find((r) => r.runId === task.runId && r.spaceId === task.spaceId) : undefined;
}

export function StateDot({ state }: { state: TaskState }) {
  const tone =
    state === "working"
      ? "bg-success"
      : state === "needs-you" || state === "stuck"
        ? "bg-warning"
        : state === "failed" || state === "didnt-start"
          ? "bg-destructive"
          : state === "done"
            ? "bg-success"
            : "bg-muted-foreground/40";
  return (
    <span className={cn("relative inline-flex size-2 shrink-0 rounded-full", tone)}>
      {state === "working" ? <span className="absolute inset-0 animate-ping rounded-full bg-success/60" /> : null}
    </span>
  );
}

/**
 * The "Ask first" Home (CUA-2346): one question and one box. With no tasks
 * yet, three real jobs sit under it; afterwards, the tasks read like
 * conversations (the one that needs you first), and your Spaces underneath.
 */
export function AskFirstHome() {
  const allTasks = useAskTasks();
  const tasks = useMemo(() => withoutStaleTasks(allTasks), [allTasks]);
  const { data: spaces } = useSpaces();
  const listed = useListedSpaces(spaces);
  const composer = useAskComposer();
  const { agents, selected, chosenAgent, agentHome, kinds, signInTo, setText, setWhere } = composer;
  useKeepHomeWarm(chosenAgent, agentHome);
  const readiness = useAgentReadiness(chosenAgent, agentHome, agents, selected);
  // The first Space being prepared after the first run: silent unless it fails.
  const prep = usePrepare();
  const onPrepareAction = usePrepareAction();

  // Home lists agents and Spaces, not tasks: with neither (a first run,
  // or tasks whose Spaces are gone), Home is the welcome again.
  const attempts = useAgentAttempts();
  const { chats, loading: chatsLoading } = useAgentChats(attempts);
  // The home being prepared since launch, and the agent waiting in it, are not "yours" yet: until a job
  // is asked for, the welcome and its suggestions stay.
  const prepared = (id: string) => Boolean(prep.state?.wanted && (id === prep.state.homeSpaceId || id === prep.state.pendingId));
  const first = prep.state?.wanted ? tasks.length === 0 && listed.counted.every((c) => prepared(c.id)) : !chats?.length && listed.counted.length === 0;
  // Nothing is drawn until the Spaces have loaded and, when there are some, their agents have answered (at most
  // 0.8 s): otherwise the welcome turns into the lists, or the agents push the Spaces down, as the page loads.
  // Once drawn, it stays drawn: a later read of the agents never blanks the page again.
  const waited = useElapsed(800);
  const shown = useRef(false);
  shown.current ||= spaces !== undefined && (waited || listed.counted.length === 0 || (chats !== null && chatsLoading === 0));
  if (!shown.current) return <div className="h-full" data-ask-first-home="" aria-busy="true" />;

  return (
    <div className="h-full scroll-y-stable" data-ask-first-home="">
      <div className={cn("mx-auto flex w-full flex-col gap-8 px-8 pb-16", first ? "max-w-[880px] pt-24" : "max-w-[1040px] pt-10")}>
        <AskBox {...composer.box} compact={!first} autoFocus />
        <AgentStatus state={agentHome ? readiness : "none"} onReconnect={() => selected && void signInTo(selected.group.app, selected.group.label)} />
        {/* Preparing is silent: only a failed prepare says so, with Try again. */}
        <PrepareLine view={prep.view} onRetry={onPrepareAction} />
        {/* After the first job: the rest of setup, once (CUA-3150, idea C). On a Mac with a permission missing, the apps row at every launch (CUA-3551). */}
        <BringTheRest />
        {first ? (
          <>
            <div className="grid gap-4 sm:grid-cols-3">
              {SUGGESTIONS.filter((s) => agentHome || kinds.some((k) => k.os === s.os) || s.os === "linux").map((s) => (
                <button
                  type="button"
                  key={s.title}
                  data-home-card=""
                  onClick={() => {
                    setText(s.prompt);
                    // Without an agent home the suggestion's kind of computer is made for it.
                    if (!agentHome) setWhere({ kind: "new", os: kinds.some((k) => k.os === s.os) ? s.os : "linux" });
                  }}
                  className="flex flex-col gap-2 rounded-xl border bg-card/60 px-5 py-4 text-left transition-colors hover:bg-accent dark:bg-white/[0.02]"
                >
                  <OsIcon os={s.os} className="size-4 text-muted-foreground" />
                  <span className="text-[14px] leading-snug font-medium">{s.title}</span>
                  {s.detail ? <span className="text-[13px] leading-snug text-muted-foreground">{s.detail}</span> : null}
                </button>
              ))}
            </div>
          </>
        ) : null}
        {/* The Agents page's own list, then the Spaces page's tiles. */}
        <YourAgents chats={chats} />
        <YourSpacesRow />
      </div>
      {composer.dialog}
    </div>
  );
}

/** True once `ms` have passed since the first render. */
function useElapsed(ms: number): boolean {
  const [done, setDone] = useState(false);
  useEffect(() => {
    const timer = setTimeout(() => setDone(true), ms);
    return () => clearTimeout(timer);
  }, [ms]);
  return done;
}
