// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Link } from "@tanstack/react-router";
import { ChevronRightIcon } from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import { useAgents, useFirstSpaceOffers, useSpaces, useSpaceThumbnail, type AgentRunSummary, type Space, type SpaceOs } from "@/bridge";
import { CuaKey } from "@/components/cua-key";
import { OsIcon } from "@/components/os-icon";
import { SpaceCanvas } from "@/components/space-canvas";
import { Button } from "@/components/ui/button";
import { mostUsedAgent, orderTasks, SUGGESTIONS, taskState, TASK_STATE_TEXT, taskTitle, useAskTasks, type AskTask, type TaskState } from "@/lib/ask-first";
import { spaceState } from "@/lib/spaces";
import { cn } from "@/lib/utils";

import { AppMark, AskBox, TASK_AGENTS, type TaskWhere } from "./ask-box";
import { useStartTask } from "./use-start-task";

/** The run a task follows, from the runs in your running Spaces. */
export function taskRun(runs: readonly AgentRunSummary[] | undefined, task: AskTask): AgentRunSummary | undefined {
  return task.runId ? runs?.find((r) => r.runId === task.runId && r.spaceId === task.spaceId) : undefined;
}

export function StateDot({ state }: { state: TaskState }) {
  const tone =
    state === "working" ? "bg-success" : state === "needs-you" || state === "stuck" ? "bg-warning" : state === "failed" ? "bg-destructive" : state === "done" ? "bg-success" : "bg-muted-foreground/40";
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
  const { data: agents, agentSetup } = useAgents();
  const { offers } = useFirstSpaceOffers();
  const start = useStartTask();
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
  const [where, setWhere] = useState<TaskWhere>({ kind: "new", os: "linux" });
  const kinds = useMemo(() => {
    const k: { os: SpaceOs; note: string; blocked: string | null }[] = offers.map((o) => ({
      os: o.os,
      note: `Free. Ready in ${o.time}${o.size ? ` (${o.size})` : ""}.`,
      blocked: o.blocked,
    }));
    return k.length ? k : [{ os: "linux" as const, note: "Free. Ready in a few minutes.", blocked: null }];
  }, [offers]);
  const running = (spaces ?? []).filter((s) => spaceState(s) === "running");
  const submit = (prompt = text, w = where) => {
    if (!prompt.trim()) return;
    start({ prompt, agent: chosenAgent, where: w });
    setText("");
  };

  const rows = orderTasks(
    tasks.map((task) => {
      const run = taskRun(agents?.runs, task);
      return { task, run, state: taskState(task, run ? { status: run.status, needsYou: Boolean(run.needsYou), lastEventMs: null } : null) };
    }),
  );
  const first = tasks.length === 0;

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
          onWhere={setWhere}
          kinds={kinds}
          spaces={running}
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
        ) : (
          <TaskList rows={rows} spaces={spaces ?? []} />
        )}
        {!first && (spaces?.length ?? 0) > 0 ? <YourSpaces spaces={spaces ?? []} /> : null}
      </div>
    </div>
  );
}

function TaskList({ rows, spaces }: { rows: { task: AskTask; run: AgentRunSummary | undefined; state: TaskState }[]; spaces: Space[] }) {
  const live = rows.filter((r) => r.state === "working" || r.state === "starting").length;
  return (
    <section>
      <h2 className="mb-3 flex items-baseline gap-2 text-[17px] font-semibold">
        Tasks {live ? <span className="text-[14px] font-normal text-muted-foreground">{live} running</span> : null}
      </h2>
      <div className="divide-y overflow-hidden rounded-xl border bg-card shadow-xs">
        {rows.map((r) => (
          <TaskRow key={r.task.id} {...r} space={spaces.find((s) => s.id === r.task.spaceId)} />
        ))}
      </div>
    </section>
  );
}

function ago(ms: number, now = Date.now()): string {
  const m = Math.round((now - ms) / 60_000);
  if (m < 2) return "Now";
  if (m < 60) return `${m} min ago`;
  const h = Math.round(m / 60);
  if (h < 24) return `${h} h ago`;
  return h < 48 ? "Yesterday" : `${Math.round(h / 24)} days ago`;
}

function TaskRow({ task, run, state, space }: { task: AskTask; run: AgentRunSummary | undefined; state: TaskState; space: Space | undefined }) {
  const thumb = useSpaceThumbnail(task.spaceId ?? "", Boolean(space) && task.ending !== "thrown");
  const line = state === "needs-you" && run?.needsYou ? run.needsYou.title : state === "failed" && task.error ? `Stopped: ${task.error}` : TASK_STATE_TEXT[state];
  const where = task.ending === "thrown" ? "Thrown away" : (space?.name ?? "");
  return (
    <Link
      to="/home/$taskId"
      params={{ taskId: task.id }}
      data-task-row={task.id}
      className={cn("flex min-h-[72px] items-center gap-4 px-5 py-3 hover:bg-accent/60", (state === "needs-you" || state === "stuck") && "border-l-2 border-l-warning")}
    >
      <AppMark agent={task.agent} size={36} />
      <div className="min-w-0 flex-1">
        <div className="truncate text-[14px] font-medium">{taskTitle(task.prompt)}</div>
        <div className="flex items-center gap-2 text-[13px] text-muted-foreground">
          <StateDot state={state} />
          <span className="truncate">{line}</span>
        </div>
      </div>
      <span className="hidden w-40 truncate text-right text-[13px] text-muted-foreground md:block">{where}</span>
      <span className="w-24 text-right text-[13px] text-muted-foreground">{ago(task.createdMs)}</span>
      <span className="hidden w-24 lg:block">
        {thumb ? <img src={thumb} alt="" className="aspect-[16/10] w-24 rounded-md border object-cover" /> : null}
      </span>
      {state === "needs-you" ? (
        <Button variant="secondary" className="h-10 px-4" render={<span />}>
          Answer
        </Button>
      ) : null}
    </Link>
  );
}

function YourSpaces({ spaces }: { spaces: Space[] }) {
  const on = spaces.filter((s) => spaceState(s) === "running").length;
  return (
    <section>
      <div className="mb-3 flex items-center justify-between">
        <h2 className="flex items-baseline gap-2 text-[17px] font-semibold">
          Your Spaces <span className="text-[14px] font-normal text-muted-foreground">{on} on</span>
        </h2>
        <Link to="/spaces" className="flex items-center gap-1 text-[13px] text-muted-foreground hover:text-foreground">
          See all {spaces.length} <ChevronRightIcon className="size-4" />
        </Link>
      </div>
      <div className="grid grid-cols-2 gap-4 md:grid-cols-4">
        {spaces.slice(0, 4).map((s) => (
          <SpaceCard key={s.id} space={s} />
        ))}
      </div>
    </section>
  );
}

function SpaceCard({ space }: { space: Space }) {
  const thumb = useSpaceThumbnail(space.id, spaceState(space) === "running");
  return (
    <Link to="/spaces/$spaceId" params={{ spaceId: space.id }} className="overflow-hidden rounded-xl border bg-card shadow-xs hover:bg-accent/60">
      <SpaceCanvas className="rounded-none">
        {thumb ? <img src={thumb} alt="" className="absolute inset-0 size-full object-cover" /> : <OsIcon os={space.os} className="size-6 text-muted-foreground" />}
      </SpaceCanvas>
      <div className="px-4 py-3">
        <div className="truncate text-[14px] font-medium">{space.name}</div>
        <div className="text-[12px] text-muted-foreground">{spaceState(space) === "running" ? "On" : "Off"}</div>
      </div>
    </Link>
  );
}
