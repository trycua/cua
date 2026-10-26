// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Link } from "@tanstack/react-router";
import { ArrowLeftIcon, ArrowUpIcon, CheckIcon, ChevronDownIcon, EyeIcon, HandIcon, Loader2Icon, Maximize2Icon, Minimize2Icon, RotateCwIcon, Trash2Icon, TriangleAlertIcon, UserPlusIcon } from "lucide-react";
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";

import { useAgents, useAgentTimeline, useSession, useShareSheet, useSpaceDetail, useSpaces, type AgentTimeline, type Space } from "@/bridge";
import { Timeline } from "@/components/agents/timeline";
import { CuaKey } from "@/components/cua-key";
import { OsIcon } from "@/components/os-icon";
import { StreamSurface } from "@/components/space-detail/stream-surface";
import { ConfirmDialog } from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { toastError } from "@/components/ui/toast";
import { keepTranscript, retryTarget, startErrorView, taskState, TASK_STATE_TEXT, taskTitle, updateAskTask, useAskTasks, type AskTask, type TaskState, type TaskWhere } from "@/lib/ask-first";
import { createProgress, spaceState } from "@/lib/spaces";
import { cn } from "@/lib/utils";

import { agentName, AppMark, WherePicker } from "./ask-box";
import { StateDot, taskRun } from "./home";
import { useWherePick } from "./this-mac-confirm";
import { useStartTask } from "./use-start-task";
import { useWhereOptions } from "./where-options";

/** What the agent is told when the person hands the computer back. */
/** How long a sent reply may take to show in the run before the thread trusts the run's status again (ms). */
const REPLY_GRACE_MS = 15_000;

export const HAND_BACK_TEXT = "I used the computer myself and have handed it back to you. Look at the screen to see what changed, then carry on.";

/**
 * One task's conversation (CUA-2346): the run's `Timeline` beside the
 * computer's live screen. Cua's own lines (getting the computer ready, a
 * failure, the ending) carry the Cua key; the agent asks in the thread when
 * it needs you, and you can take over the screen and hand it back.
 */
export function AskFirstTask({ taskId }: { taskId: string }) {
  const task = useAskTasks().find((t) => t.id === taskId);
  if (!task) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-3 text-center">
        <p className="text-[14px] text-muted-foreground">This task isn't on this device.</p>
        <Button variant="outline" render={<Link to="/home" />}>
          Back to Home
        </Button>
      </div>
    );
  }
  return <TaskView task={task} />;
}

function TaskView({ task }: { task: AskTask }) {
  const { data: spaces, deleteSpace } = useSpaces();
  const { data: agents, sendMessage } = useAgents();
  const { data: session } = useSession();
  const sharing = useShareSheet();
  const { retry } = useStartTask();
  // On this Mac there is no other screen to show, share or take over, and nothing to throw away.
  const space = task.onHost ? undefined : spaces?.find((s) => s.id === task.spaceId);
  const pending = task.pendingId ? spaces?.find((s) => s.id === task.pendingId) : undefined;
  const ref = task.runId && task.spaceId && task.ending !== "thrown" ? { spaceId: task.spaceId, runId: task.runId } : null;
  const live = useAgentTimeline(ref);
  const saved = useMemo<AgentTimeline | null>(
    () =>
      task.transcript
        ? { items: task.transcript, status: task.lastStatus, phase: "", caughtUp: true, isLoading: false, error: null, unsupported: false, lastEventMs: null }
        : null,
    [task.transcript, task.lastStatus],
  );
  const timeline = saved ?? live;
  const run = taskRun(agents?.runs, task);
  // A reply just sent reads as working until the run writes its new turn
  // (otherwise the old "done" or ask would show for a moment).
  const [sentMs, setSentMs] = useState<number | null>(null);
  const [, tick] = useState(0);
  const awaiting = sentMs !== null && (timeline.lastEventMs ?? 0) < sentMs && Date.now() - sentMs < REPLY_GRACE_MS;
  useEffect(() => {
    if (!awaiting) return;
    const t = setTimeout(() => tick((n) => n + 1), REPLY_GRACE_MS);
    return () => clearTimeout(t);
  }, [awaiting]);
  const read = taskState(task, ref ? { status: timeline.status ?? run?.status ?? null, needsYou: Boolean(run?.needsYou), lastEventMs: timeline.lastEventMs } : null);
  const state: TaskState = awaiting && (read === "done" || read === "needs-you" || read === "kept") ? "working" : read;
  const [mode, setMode] = useState<"watch" | "control">("watch");
  const [big, setBig] = useState(false);
  const [throwing, setThrowing] = useState(false);
  const [reply, setReply] = useState("");
  const replyRef = useRef<HTMLTextAreaElement>(null);
  const name = agentName(task.agent);

  // A thrown-away or failed task has no screen to control.
  useEffect(() => {
    if (!space && mode === "control") setMode("watch");
  }, [space, mode]);

  const send = async (text: string) => {
    if (!ref || !text.trim()) return;
    setReply("");
    setSentMs(Date.now());
    await sendMessage(ref, text.trim()).catch(toastError(`Couldn't send that to ${name}`));
  };
  const takeOver = () => {
    setMode("control");
    setBig(true);
  };
  const handBack = () => {
    setMode("watch");
    setBig(false);
    void send(HAND_BACK_TEXT);
  };
  const keep = () => updateAskTask(task.id, { ending: "kept", endedMs: Date.now() });
  const throwAway = async () => {
    if (!task.spaceId) return;
    updateAskTask(task.id, { transcript: keepTranscript(timeline.items), lastStatus: timeline.status, ending: "thrown", endedMs: Date.now() });
    await deleteSpace(task.spaceId).catch(toastError("Couldn't throw the computer away"));
  };

  const footer = (
    <ThreadFooter
      task={task}
      state={state}
      timeline={timeline}
      name={name}
      needsYou={run?.needsYou ?? null}
      onTakeOver={space ? takeOver : undefined}
      onAnswer={() => replyRef.current?.focus()}
      onKeep={keep}
      onThrow={() => setThrowing(true)}
      onRetry={(where) => retry(task, where)}
    />
  );
  const hasThread = timeline.items.length > 0;

  return (
    <div className="flex h-full flex-col" data-ask-first-task={task.id} data-task-state={state}>
      <header className="flex h-16 shrink-0 items-center gap-3 border-b px-8">
        <Button variant="ghost" className="-ml-2 h-9" render={<Link to="/home" />}>
          <ArrowLeftIcon /> Home
        </Button>
        <AppMark agent={task.agent} size={32} />
        <div className="min-w-0 flex-1">
          <h2 className="truncate text-[15px] font-semibold">{taskTitle(task.prompt)}</h2>
          <p className="flex items-center gap-2 text-[13px] text-muted-foreground">
            <StateDot state={state} /> {TASK_STATE_TEXT[state]}
          </p>
        </div>
        {space ? (
          <Button variant="outline" className="h-10" onClick={() => void sharing.open(space, session?.signedIn ?? false).catch(toastError("Couldn't share"))}>
            <UserPlusIcon /> Share
          </Button>
        ) : null}
      </header>
      {mode === "control" ? (
        <div className="flex min-h-12 shrink-0 items-center gap-3 border-b bg-brand-surface px-8 text-[13px]" role="status">
          <HandIcon className="size-4" />
          <span className="flex-1">You have the controls. {name} waits until you hand it back.</span>
          <Button className="h-9" onClick={handBack}>
            Hand back to {name}
          </Button>
        </div>
      ) : null}
      <div className="flex min-h-0 flex-1">
        <div className={cn("flex min-w-0 flex-col", big ? "w-[420px] shrink-0 border-r" : "flex-1")}>
          {hasThread ? (
            <Timeline className="flex-1" timeline={timeline} footer={footer} />
          ) : (
            <div className="min-h-0 flex-1 overflow-y-auto">
              <div className="mx-auto flex max-w-[720px] flex-col justify-end gap-6 px-8 pt-8 pb-6" style={{ minHeight: "100%" }}>
                <div className="flex justify-end">
                  <div className="max-w-[85%] rounded-2xl rounded-br-md bg-muted px-3.5 py-2 text-[13px] leading-[1.55] whitespace-pre-wrap">{task.prompt}</div>
                </div>
                {task.error ? null : <Starting pending={pending} space={space} name={name} onHost={Boolean(task.onHost)} />}
                {footer}
              </div>
            </div>
          )}
          {task.ending !== "thrown" ? (
            <div className="shrink-0 px-8 pb-6">
              <div className="mx-auto flex min-h-14 max-w-[720px] items-end gap-2 rounded-2xl border bg-card py-1.5 pr-1.5 pl-5 shadow-xs dark:bg-white/[0.03]">
                <textarea
                  ref={replyRef}
                  value={reply}
                  disabled={!ref}
                  rows={1}
                  onChange={(e) => setReply(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
                      e.preventDefault();
                      void send(reply);
                    }
                  }}
                  aria-label={`Reply to ${name}`}
                  placeholder={state === "needs-you" ? "Or answer in words" : state === "done" || state === "kept" ? "Ask a follow-up" : `Reply to ${name}`}
                  className="min-h-11 flex-1 resize-none self-center bg-transparent py-2.5 text-[14px] outline-none placeholder:text-muted-foreground disabled:opacity-60"
                />
                <button
                  type="button"
                  aria-label="Send"
                  disabled={!ref || !reply.trim()}
                  onClick={() => void send(reply)}
                  className={cn("flex size-11 shrink-0 items-center justify-center rounded-full", ref && reply.trim() ? "bg-primary text-primary-foreground" : "bg-foreground/10 text-muted-foreground")}
                >
                  <ArrowUpIcon className="size-5" />
                </button>
              </div>
            </div>
          ) : null}
        </div>
        <aside className={cn("flex shrink-0 flex-col gap-4 overflow-y-auto px-6 pt-8 pb-6", big ? "min-w-0 flex-1" : "w-[440px] border-l")}>
          <Screen
            task={task}
            space={space}
            state={state}
            mode={mode}
            big={big}
            onMode={(m) => (m === "control" ? takeOver() : mode === "control" ? handBack() : setMode("watch"))}
            onBig={() => setBig((b) => !b)}
          />
          <ComputerCard task={task} space={space} />
        </aside>
      </div>
      <ConfirmDialog
        open={throwing}
        onOpenChange={setThrowing}
        title="Throw the computer away?"
        description="This deletes the computer and everything on it. This conversation stays here."
        confirmLabel="Throw it away"
        destructive
        onConfirm={() => void throwAway()}
      />
    </div>
  );
}

function CuaLine({ children }: { children: ReactNode }) {
  return (
    <div className="flex items-start gap-3 rounded-xl border bg-card px-5 py-4 shadow-xs">
      <CuaKey size={28} />
      <div className="min-w-0 flex-1 text-[13px] leading-relaxed text-muted-foreground">{children}</div>
    </div>
  );
}

function Starting({ pending, space, name, onHost }: { pending: Space | undefined; space: Space | undefined; name: string; onHost: boolean }) {
  const making = pending?.progress;
  const steps = onHost
    ? [{ text: `Opening ${name} on this Mac`, done: false, now: true }]
    : [
        { text: "Making the computer", done: Boolean(space), now: !space },
        { text: `Opening ${name} inside it`, done: false, now: Boolean(space) },
      ];
  return (
    <div className="rounded-xl border bg-card px-5 py-4 shadow-xs" data-ask-starting="">
      <div className="flex items-center gap-3">
        <CuaKey size={28} />
        <div className="flex-1">
          <div className="text-[15px] font-semibold">{onHost ? "Starting on this Mac" : "Getting a fresh computer ready"}</div>
          <div className="text-[13px] text-muted-foreground">
            {onHost
              ? "It works with your own files and apps."
              : (making?.transfer ??
                (making ? `${making.label} ${Math.round(createProgress({ progress: making }) * 100)}%` : "Usually under a minute. The first one takes a few minutes."))}
          </div>
        </div>
      </div>
      <ol className="mt-4 space-y-3">
        {steps.map((s) => (
          <li key={s.text} className="flex items-center gap-3 text-[14px]">
            {s.done ? <CheckIcon className="size-4 text-success" /> : s.now ? <Loader2Icon className="size-4 animate-spin text-brand-strong" /> : <span className="size-4" />}
            <span className={cn(!s.done && !s.now && "text-muted-foreground")}>{s.text}</span>
          </li>
        ))}
      </ol>
    </div>
  );
}

/**
 * A start that failed, in plain words: what went wrong, Try again and
 * Change computer right in the card, and the core's own words under
 * Details.
 */
export function StartFailed({ error, name, host, onRetry }: { error: string; name: string; host: boolean; onRetry: (where?: TaskWhere) => void }) {
  const view = startErrorView(error, name);
  const { kinds, spaces } = useWhereOptions();
  const [open, setOpen] = useState(false);
  const pick = useWherePick(name);
  return (
    <CuaLine>
      <div data-ask-start-failed={view.action}>
        <div className="mb-1 flex items-center gap-2 text-[15px] font-semibold text-foreground">
          <TriangleAlertIcon className="size-4 text-destructive" /> {view.title}
        </div>
        <p>{view.body}</p>
        <div className="mt-3 flex flex-wrap items-center gap-2">
          {view.action === "set-up-host" ? (
            <Button className="h-10 px-4" render={<Link to="/machines" />}>
              Open Computers
            </Button>
          ) : null}
          {view.action === "add-key" ? (
            <Button className="h-10 px-4" render={<Link to="/settings/connected-apps" />}>
              Add your key
            </Button>
          ) : null}
          {view.retry ? (
            <Button variant={view.action === "retry" ? "default" : "outline"} className="h-10 px-4" onClick={() => (host ? pick.choose({ kind: "this-mac" }, onRetry) : onRetry())}>
              <RotateCwIcon /> Try again
            </Button>
          ) : null}
          <WherePicker
            where={null}
            onWhere={(w) => pick.choose(w, onRetry, () => setOpen(true))}
            open={open}
            onOpenChange={setOpen}
            kinds={kinds}
            spaces={spaces}
            trigger={
              <Button variant={view.action === "change-computer" ? "default" : "outline"} className="h-10 px-4" data-ask-change-computer="">
                Change computer <ChevronDownIcon />
              </Button>
            }
          />
        </div>
        <details className="mt-3 text-[12px]">
          <summary className="cursor-pointer select-none">Details</summary>
          <p className="mt-1 font-mono break-words whitespace-pre-wrap">{error}</p>
        </details>
      </div>
      {pick.dialog}
    </CuaLine>
  );
}

function ThreadFooter({
  task,
  state,
  timeline,
  name,
  needsYou,
  onTakeOver,
  onAnswer,
  onKeep,
  onThrow,
  onRetry,
}: {
  task: AskTask;
  state: TaskState;
  timeline: AgentTimeline;
  name: string;
  needsYou: { title: string; detail: string } | null;
  onTakeOver?: () => void;
  onAnswer: () => void;
  onKeep: () => void;
  onThrow: () => void;
  onRetry: (where?: TaskWhere) => void;
}) {
  return (
    <div className="flex flex-col gap-5" data-ask-footer="">
      {task.error ? <StartFailed error={task.error} name={name} host={retryTarget(task).kind === "host"} onRetry={onRetry} /> : null}
      {state === "working" ? (
        <p className="flex items-center gap-2 text-xs text-muted-foreground" role="status">
          <StateDot state="working" />
          {timeline.phase === "responding" ? "Writing" : timeline.phase === "tool" ? "Using the computer" : "Working"}
        </p>
      ) : null}
      {state === "stuck" ? (
        <CuaLine>
          <div className="mb-1 text-[15px] font-semibold text-foreground">No progress for 10 minutes</div>
          {name} hasn't done anything new for a while. You can nudge it with a reply, or take over the screen.
          {onTakeOver ? (
            <div className="mt-3">
              <Button variant="outline" className="h-10" onClick={onTakeOver}>
                <HandIcon /> Take over
              </Button>
            </div>
          ) : null}
        </CuaLine>
      ) : null}
      {state === "needs-you" && needsYou ? (
        <div className="rounded-xl border bg-card px-5 py-4 shadow-xs" data-ask-needs-you="">
          <div className="flex items-start gap-3">
            <AppMark agent={task.agent} size={28} />
            <div className="min-w-0 flex-1">
              <div className="text-[15px] font-semibold">{needsYou.title}</div>
              <div className="mt-1 text-[13px] leading-relaxed text-muted-foreground">{needsYou.detail}</div>
              <div className="mt-4 flex flex-wrap gap-2">
                {onTakeOver ? (
                  <Button className="h-11 px-4" onClick={onTakeOver}>
                    <HandIcon /> Do it on the screen
                  </Button>
                ) : null}
                <Button variant="outline" className="h-11 px-4" onClick={onAnswer}>
                  Answer in words
                </Button>
              </div>
            </div>
          </div>
        </div>
      ) : null}
      {state === "done" ? (
        <div className="rounded-xl border bg-card shadow-xs" data-ask-done="">
          <div className="flex items-start gap-3 px-5 py-4">
            <CheckIcon className="mt-0.5 size-5 text-success" />
            <div>
              <div className="text-[15px] font-semibold">{name} finished</div>
              <div className="mt-1 text-[13px] text-muted-foreground">
                {task.onHost ? "Ask a follow-up below." : "Ask a follow-up below, or decide what happens to the computer."}
              </div>
            </div>
          </div>
          {task.onHost ? null : (
          <div className="flex flex-wrap items-center gap-3 border-t px-5 py-3">
            <span className="flex-1 text-[13px] text-muted-foreground">The computer is still on. What should happen to it?</span>
            <Button variant="outline" className="h-11 px-4" onClick={onKeep}>
              Keep it
            </Button>
            <Button variant="outline" className="h-11 px-4" onClick={onThrow}>
              <Trash2Icon /> Throw it away
            </Button>
          </div>
          )}
        </div>
      ) : null}
      {state === "done" && !task.onHost ? <p className="text-[12px] text-muted-foreground">Throwing it away deletes the computer. This conversation stays.</p> : null}
      {state === "kept" ? <p className="text-[12px] text-muted-foreground">You kept the computer. It's with your Spaces.</p> : null}
      {state === "thrown" ? <p className="text-[12px] text-muted-foreground">You threw the computer away. This is the conversation as it was.</p> : null}
    </div>
  );
}

function Screen({
  task,
  space,
  state,
  mode,
  big,
  onMode,
  onBig,
}: {
  task: AskTask;
  space: Space | undefined;
  state: TaskState;
  mode: "watch" | "control";
  big: boolean;
  onMode: (m: "watch" | "control") => void;
  onBig: () => void;
}) {
  const detail = useSpaceDetail(space, null).detail;
  // A task that never started shows no screen: there is nothing of its to watch.
  const on = space && state !== "didnt-start" ? spaceState(space) === "running" : false;
  const caption =
    state === "didnt-start"
      ? TASK_STATE_TEXT["didnt-start"]
      : !space
        ? task.ending === "thrown"
          ? "Thrown away"
          : task.onHost
            ? state === "working" || state === "stuck"
              ? "Working on this Mac"
              : TASK_STATE_TEXT[state]
            : "Not ready yet"
        : state === "needs-you"
          ? "Waiting for you"
          : state === "working" || state === "stuck"
            ? "Live"
            : state === "failed"
              ? TASK_STATE_TEXT.failed
              : "Finished";
  return (
    <div className={cn("overflow-hidden rounded-xl border bg-card shadow-xs", state === "needs-you" && mode === "watch" && "ring-2 ring-warning/70", mode === "control" && "ring-2 ring-brand")} data-ask-screen={mode}>
      {space && on ? (
        <StreamSurface
          spaceId={space.id}
          os={space.os}
          canStream={detail?.canStream ?? false}
          previewText={detail?.previewText ?? ""}
          interactive={mode === "control"}
          className="aspect-[16/10] w-full rounded-none border-0"
        />
      ) : (
        <div className="flex aspect-[16/10] items-center justify-center bg-thumb text-[13px] text-muted-foreground">
          <OsIcon os={task.os} className="mr-2 size-5" />
          {caption}
        </div>
      )}
      <div className="flex min-h-12 items-center gap-2 px-3">
        <span className="min-w-0 flex-1 truncate text-[13px] text-muted-foreground">{caption}</span>
        {space && on ? (
          <>
            <div className="flex rounded-lg border p-0.5 text-[13px]" role="group" aria-label="Screen">
              <button
                type="button"
                aria-pressed={mode === "watch"}
                onClick={() => onMode("watch")}
                className={cn("flex h-8 items-center gap-1.5 rounded-md px-2.5", mode === "watch" && "bg-foreground/[0.08] font-medium")}
              >
                <EyeIcon className="size-3.5" /> Watch
              </button>
              <button
                type="button"
                aria-pressed={mode === "control"}
                onClick={() => onMode("control")}
                className={cn("flex h-8 items-center gap-1.5 rounded-md px-2.5", mode === "control" && "bg-brand-surface font-medium")}
              >
                <HandIcon className="size-3.5" /> Take over
              </button>
            </div>
            <Button variant="ghost" size="icon" aria-label={big ? "Make smaller" : "Make bigger"} onClick={onBig}>
              {big ? <Minimize2Icon /> : <Maximize2Icon />}
            </Button>
          </>
        ) : null}
      </div>
    </div>
  );
}

function ComputerCard({ task, space }: { task: AskTask; space: Space | undefined }) {
  const os = task.os === "macos" ? "Mac" : task.os === "windows" ? "Windows" : "Linux";
  const line = task.onHost
    ? "Your own Mac, with your files and apps"
    : !space
      ? task.ending === "thrown"
        ? "Thrown away"
        : task.error
          ? "Not made"
          : "Getting ready"
      : spaceState(space) === "running"
        ? "On"
        : "Off";
  return (
    <div className="rounded-xl border bg-card text-[13px] shadow-xs">
      <div className="flex min-h-12 items-center gap-3 px-4 py-2">
        <OsIcon os={task.os} className="size-4 text-muted-foreground" />
        <div className="min-w-0 flex-1">
          <div className="font-medium">{task.onHost ? "This Mac" : space ? space.name : `${os} computer for this task`}</div>
          <div className="text-muted-foreground">{line}</div>
        </div>
        {space ? (
          <Button variant="ghost" size="sm" render={<Link to="/spaces/$spaceId" params={{ spaceId: space.id }} />}>
            Open
          </Button>
        ) : null}
      </div>
    </div>
  );
}
