// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect, useState } from "react";

import { useAgentChats, useAgents, useSpaces, type AgentRunSummary } from "@/bridge";
import { CuaKey } from "@/components/cua-key";
import { useAgentAttempts } from "@/hooks/use-agent-attempts";
import { OsIcon } from "@/components/os-icon";
import { currentWhere, mostUsedAgent, SUGGESTIONS, useAskTasks, type AskTask, type TaskState } from "@/lib/ask-first";
import { useListedSpaces } from "@/components/spaces/listed";
import { cn } from "@/lib/utils";

import { agentName, AskBox, TASK_AGENTS, type TaskWhere } from "./ask-box";
import { YourAgents, YourSpaces as YourSpacesRow } from "./home-sections";
import { useWherePick } from "./this-mac-confirm";
import { useStartTask } from "./use-start-task";
import { useWhereOptions } from "./where-options";

/** The run a task follows, from the runs in your running Spaces. */
export function taskRun(runs: readonly AgentRunSummary[] | undefined, task: AskTask): AgentRunSummary | undefined {
  return task.runId ? runs?.find((r) => r.runId === task.runId && r.spaceId === task.spaceId) : undefined;
}

export function StateDot({ state }: { state: TaskState }) {
  const tone =
    state === "working" ? "bg-success" : state === "needs-you" || state === "stuck" ? "bg-warning" : state === "failed" || state === "didnt-start" ? "bg-destructive" : state === "done" ? "bg-success" : "bg-muted-foreground/40";
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
  const tasks = useAskTasks();
  const { data: spaces } = useSpaces();
  const listed = useListedSpaces(spaces);
  const { agentSetup } = useAgents();
  const { kinds, spaces: pickable } = useWhereOptions();
  const { start } = useStartTask();
  const [installed, setInstalled] = useState<ReadonlySet<string>>(new Set());
  useEffect(() => {
    let live = true;
    agentSetup()
      .then((rows) => {
        if (!live) return;
        const found = new Set(rows.filter((r) => r.installed).map((r) => r.agent));
        setInstalled(new Set(TASK_AGENTS.filter((a) => found.has(a.setupId)).map((a) => a.id)));
      })
      .catch(() => {});
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const mostUsed = mostUsedAgent(tasks);
  const [text, setText] = useState("");
  const [agent, setAgent] = useState<string | null>(null);
  const chosenAgent = agent ?? mostUsed ?? TASK_AGENTS.find((a) => installed.has(a.id))?.id ?? "claude-code";
  // A new Linux computer unless you pick; a pick that's no longer offered falls back to it.
  const [picked, setWhere] = useState<TaskWhere | null>(null);
  const [whereOpen, setWhereOpen] = useState(false);
  const pick = useWherePick(agentName(chosenAgent));
  const where = currentWhere(picked, kinds, pickable);
  const submit = (prompt = text, w = where) => {
    if (!prompt.trim()) return;
    start({ prompt, agent: chosenAgent, where: w });
    setText("");
  };

  // Home lists agents and Spaces, not tasks: with neither (a first run,
  // or tasks whose Spaces are gone), Home is the welcome again.
  const attempts = useAgentAttempts();
  const { chats } = useAgentChats(attempts);
  const first = !chats?.length && listed.counted.length === 0;

  return (
    <div className="h-full overflow-y-auto [scrollbar-gutter:stable]" data-ask-first-home="">
      <div className={cn("mx-auto flex w-full flex-col gap-8 px-8 pb-16", first ? "max-w-[880px] pt-24" : "max-w-[1040px] pt-10")}>
        {first ? <h1 className="font-heading text-[40px] font-semibold tracking-tight">What do you want to do?</h1> : null}
        <AskBox
          text={text}
          onText={setText}
          agent={chosenAgent}
          onAgent={setAgent}
          installed={installed}
          mostUsed={mostUsed}
          where={where}
          onWhere={(w) => pick.choose(w, setWhere, () => setWhereOpen(true))}
          whereOpen={whereOpen}
          onWhereOpen={setWhereOpen}
          kinds={kinds}
          spaces={pickable}
          busy={false}
          onSubmit={() => submit()}
          compact={!first}
          autoFocus
        />
        {first ? (
          <>
            <div className="grid gap-4 sm:grid-cols-3">
              {SUGGESTIONS.filter((s) => kinds.some((k) => k.os === s.os) || s.os === "linux").map((s) => (
                <button
                  type="button"
                  key={s.title}
                  onClick={() => {
                    setText(s.prompt);
                    setWhere({ kind: "new", os: kinds.some((k) => k.os === s.os) ? s.os : "linux" });
                  }}
                  className="flex flex-col gap-2 rounded-xl border bg-card/60 px-5 py-4 text-left transition-colors hover:bg-accent dark:bg-white/[0.02]"
                >
                  <OsIcon os={s.os} className="size-4 text-muted-foreground" />
                  <span className="text-[14px] leading-snug font-medium">{s.title}</span>
                  <span className="text-[13px] leading-snug text-muted-foreground">{s.detail}</span>
                </button>
              ))}
            </div>
            <p className="flex items-center gap-3 text-[13px] text-muted-foreground">
              <CuaKey size={24} />A fresh computer does the work, so yours stays untouched. Watch it live, step in any time, and throw it away when you're done.
            </p>
          </>
        ) : null}
        {/* The Agents page's own list, then the Spaces page's tiles. */}
        <YourAgents chats={chats} />
        <YourSpacesRow />
      </div>
      {pick.dialog}
    </div>
  );
}
