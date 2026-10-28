// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The "Ask first" Home's tasks (CUA-2346, behind `spaces_ask_first_home`):
 * one job asked for on Home, the computer made for it, and the coding
 * agent's run on that computer.
 *
 * A task is kept on this device (`localStorage`), so it outlives the page and
 * the computer: when the person throws the computer away, the conversation
 * so far is copied into the task first. Syncing tasks across devices and
 * teammates needs a task record in the control plane (a follow-up).
 */

import { useSyncExternalStore } from "react";

import type { AgentRunStatus, HostStatus, Space, SpaceOs, TranscriptItem } from "@/bridge";
import type { DataAdapter } from "@/bridge/adapter";
import { threadOpAvailable } from "@/bridge/ops/thread";

import { isMachineDesktop, orderSpaces, spaceState } from "./spaces";

export type TaskEnding = "kept" | "thrown";

export interface AskTask {
  id: string;
  /** What the person asked for, as typed. */
  prompt: string;
  /** Harness id (`claude-code`, `openai-codex`, ...). */
  agent: string;
  os: SpaceOs;
  createdMs: number;
  /** The create's row while the computer is being made. */
  pendingId: string | null;
  /** The computer, once made (or the one picked). */
  spaceId: string | null;
  /** The agent's run, once started. */
  runId: string | null;
  /** Why making the computer or starting the agent failed. */
  error: string | null;
  ending: TaskEnding | null;
  endedMs: number | null;
  /** The conversation, copied when the computer was thrown away. */
  transcript: TranscriptItem[] | null;
  /** The status the run last had, for a thrown-away task's line. */
  lastStatus: AgentRunStatus | null;
  /** It runs on this Mac itself (its own Space id is in `spaceId`): nothing to throw away. */
  onHost?: boolean;
  /** Started in its Space's thread (the redesign): the job is the Space's first message, @mentioning the agent. */
  inSpace?: boolean;
}

const KEY = "cua.askFirst.tasks";
/** The newest tasks kept; older ones drop off. */
const MAX_TASKS = 50;
/** Conversation items kept per thrown-away task. */
const MAX_ITEMS = 300;

let cache: AskTask[] | null = null;
const listeners = new Set<() => void>();

function load(): AskTask[] {
  if (cache) return cache;
  try {
    const raw = globalThis.localStorage?.getItem(KEY);
    const parsed = raw ? (JSON.parse(raw) as unknown) : [];
    cache = Array.isArray(parsed) ? (parsed as AskTask[]) : [];
  } catch {
    cache = [];
  }
  return cache;
}

function save(next: AskTask[]): void {
  cache = next.slice(0, MAX_TASKS);
  try {
    globalThis.localStorage?.setItem(KEY, JSON.stringify(cache));
  } catch {
    // Not kept past this session (storage full or off).
  }
  for (const l of [...listeners]) l();
}

/** Every task, newest first. */
/**
 * Records a line of the task's conversation in its Space's thread, in core:
 * the Space keeps it, on every device and for the CLI, SDK and MCP, while
 * this device only remembers which Space and run a task is. Best effort: a
 * host without the thread still runs the task.
 */
export async function postToSpaceThread(data: Pick<DataAdapter, "call" | "mode"> | null | undefined, spaceId: string | null | undefined, text: string): Promise<void> {
  if (!data || !spaceId || !text.trim() || !threadOpAvailable(data.mode, "thread.post")) return;
  await data.call("thread.post", { spaceId, text: text.trim() }).catch(() => {});
}

export function askTasks(): AskTask[] {
  return load();
}

export function askTask(id: string): AskTask | undefined {
  return load().find((t) => t.id === id);
}

export function addAskTask(t: Pick<AskTask, "prompt" | "agent" | "os"> & Partial<AskTask>, now = Date.now()): AskTask {
  const task: AskTask = {
    id: `task-${now.toString(36)}-${Math.random().toString(36).slice(2, 6)}`,
    createdMs: now,
    pendingId: null,
    spaceId: null,
    runId: null,
    error: null,
    ending: null,
    endedMs: null,
    transcript: null,
    lastStatus: null,
    ...t,
  };
  save([task, ...load()]);
  return task;
}

export function updateAskTask(id: string, patch: Partial<AskTask> | ((t: AskTask) => Partial<AskTask>)): void {
  save(load().map((t) => (t.id === id ? { ...t, ...(typeof patch === "function" ? patch(t) : patch) } : t)));
}

/** Keeps the conversation with the task before its computer goes. */
export function keepTranscript(items: readonly TranscriptItem[]): TranscriptItem[] {
  return items.slice(-MAX_ITEMS).map((i) => ({ ...i, steps: [...i.steps] }));
}

/** For tests: forget the cached list. */
export function resetAskTasks(): void {
  cache = null;
}

const subscribe = (l: () => void) => {
  listeners.add(l);
  const onStorage = (e: StorageEvent) => {
    if (e.key === null || e.key === KEY) {
      cache = null;
      l();
    }
  };
  globalThis.addEventListener?.("storage", onStorage);
  return () => {
    listeners.delete(l);
    globalThis.removeEventListener?.("storage", onStorage);
  };
};

export function useAskTasks(): AskTask[] {
  return useSyncExternalStore(subscribe, load, load);
}

/** The coding app used most for tasks on this device, or null before the first. */
export function mostUsedAgent(tasks: readonly AskTask[]): string | null {
  const counts = new Map<string, number>();
  for (const t of tasks) counts.set(t.agent, (counts.get(t.agent) ?? 0) + 1);
  let best: string | null = null;
  for (const [agent, n] of counts) if (best === null || n > counts.get(best)!) best = agent;
  return best;
}

/** A task's title: its first line, short. */
export function taskTitle(prompt: string, max = 80): string {
  const line = prompt.split("\n").map((l) => l.trim()).find(Boolean) ?? "";
  return [...line].length <= max ? line : `${[...line].slice(0, max - 1).join("").trimEnd()}…`;
}

/** Where the job runs, as picked: a new computer of a kind on this Mac, a Space you already have, or this Mac itself. */
export type TaskWhere = { kind: "new"; os: SpaceOs } | { kind: "space"; space: Space } | { kind: "this-mac" };

/**
 * What `agents.start` is pointed at. The core's `agent_start` only takes a
 * Space id (`local:`, `cloud:`, `direct:`, `relay:`), so this Mac (the
 * `this-mac` row) is first turned into its own Space id: the one its host
 * service shares it as (`hostSpaceId`), the same id `cua mcp` treats as
 * This Mac. The person confirms once that the agent works on their own
 * files and apps.
 */
export type TaskTarget = { kind: "create"; os: SpaceOs } | { kind: "space"; spaceId: string } | { kind: "host" };

/** The id of this Mac's own row in the Spaces list. */
export const THIS_MAC_ID = "this-mac";

/** The raw error kept on a task for this Mac when its host service doesn't share it (no Space id to start in). */
export const HOST_NOT_SHARED = `${THIS_MAC_ID}: this Mac isn't shared by its host service, so it has no Space id to start in`;

/**
 * This Mac's own Space id, from its host service (`host.status`): the relay
 * machine (`relay:<machine>`) or the direct address (`direct:<host:port>`)
 * it shares its desktop on. Null when it isn't set up or doesn't share its
 * desktop.
 */
export function hostSpaceId(status: Pick<HostStatus, "configured" | "mode" | "machineId" | "directUrl" | "sharing" | "shareDesktop"> | null | undefined): string | null {
  if (!status?.configured || !status.sharing || status.shareDesktop === false) return null;
  if (status.mode === "relay" && status.machineId) return `relay:${status.machineId}`;
  if (status.mode === "direct" && status.directUrl) {
    try {
      return `direct:${new URL(status.directUrl).host}`;
    } catch {
      return null;
    }
  }
  return null;
}

export function taskTarget(where: TaskWhere): TaskTarget {
  if (where.kind === "new") return { kind: "create", os: where.os };
  if (where.kind === "this-mac" || where.space.id === THIS_MAC_ID) return { kind: "host" };
  return { kind: "space", spaceId: where.space.id };
}

export const whereOs = (where: TaskWhere): SpaceOs => (where.kind === "new" ? where.os : where.kind === "space" ? where.space.os : "macos");

/** A kept task's target, for Try again: its computer if it has one (the row, when listed, decides), else a new one. */
export function retryTarget(task: Pick<AskTask, "spaceId" | "os" | "onHost">): TaskTarget {
  if (task.onHost || task.spaceId === THIS_MAC_ID) return { kind: "host" };
  if (!task.spaceId) return { kind: "create", os: task.os };
  return { kind: "space", spaceId: task.spaceId };
}

/** A kind of new computer this Mac can make, with its line ("Free. Ready in about 2 min."). */
export interface NewKind {
  os: SpaceOs;
  note: string;
  blocked: string | null;
}

/** The Spaces you already have that a job can use: running ones, not a machine's own desktop (this Mac has its own row). */
export function pickableSpaces(spaces: readonly Space[] | undefined): Space[] {
  return orderSpaces((spaces ?? []).filter((s) => !isMachineDesktop(s) && spaceState(s) === "running"));
}

/** Where a job runs unless you pick: a new Linux computer on this Mac (or the first kind this Mac can make). */
export function defaultWhere(kinds: readonly NewKind[]): TaskWhere {
  const ok = kinds.filter((k) => !k.blocked);
  const os = ok.find((k) => k.os === "linux")?.os ?? ok[0]?.os ?? "linux";
  return { kind: "new", os };
}

/** The pick if it's still offered, else the default (a picked Space that went away falls back). */
export function currentWhere(picked: TaskWhere | null, kinds: readonly NewKind[], spaces: readonly Space[]): TaskWhere {
  if (picked?.kind === "this-mac") return picked;
  if (picked?.kind === "new" && kinds.some((k) => k.os === picked.os && !k.blocked)) return picked;
  if (picked?.kind === "space") {
    const row = spaces.find((s) => s.id === picked.space.id);
    if (row) return { kind: "space", space: row };
  }
  return defaultWhere(kinds);
}

const THIS_MAC_OK_KEY = "cua.askFirst.thisMacOk";
const okListeners = new Set<() => void>();

/** Whether the person said once, on this device, that tasks may work on this Mac itself. */
export function thisMacConfirmed(): boolean {
  try {
    return globalThis.localStorage?.getItem(THIS_MAC_OK_KEY) === "1";
  } catch {
    return false;
  }
}

/** Remembers (or forgets, from Settings) that tasks may work on this Mac itself. */
export function setThisMacConfirmed(ok: boolean): void {
  try {
    if (ok) globalThis.localStorage?.setItem(THIS_MAC_OK_KEY, "1");
    else globalThis.localStorage?.removeItem(THIS_MAC_OK_KEY);
  } catch {
    // Asked again next time.
  }
  for (const l of [...okListeners]) l();
}

export function useThisMacConfirmed(): boolean {
  return useSyncExternalStore(
    (l) => {
      okListeners.add(l);
      return () => okListeners.delete(l);
    },
    thisMacConfirmed,
    thisMacConfirmed,
  );
}

/** What the person is told, once, before a task works on this Mac itself. */
export function thisMacConfirmText(agent: string): { title: string; description: string } {
  return {
    title: "Work on this Mac?",
    description: `${agent} will work directly on this Mac, with your files and apps. It can't be undone by throwing the computer away. ${agent} doesn't ask before each step here, the same as on a new computer.`,
  };
}

/** What a failed start offers in its card. */
export type StartErrorAction = "change-computer" | "add-key" | "retry" | "set-up-host";

export interface StartErrorView {
  title: string;
  body: string;
  /** The one action that helps most. */
  action: StartErrorAction;
  /** Whether Try again can help (not for a target that can't work). */
  retry: boolean;
}

/**
 * A core error from making the computer or starting the agent, in plain
 * words with the one thing to do next. The raw text stays on the task and
 * shows under Details.
 */
export function startErrorView(raw: string, agent: string): StartErrorView {
  const e = raw.toLowerCase();
  if (e.includes("isn't shared"))
    return {
      title: "This Mac isn't set up for tasks yet",
      body: "Turn on sharing for this Mac on the Machines page, then try again. Or pick another machine.",
      action: "set-up-host",
      retry: true,
    };
  // A task from before this Mac was turned into its own Space id: trying again now works.
  if (e.includes(`"${THIS_MAC_ID}"`))
    return { title: "Couldn't start on this Mac", body: "Try again to start it on this Mac, or pick another computer.", action: "retry", retry: true };
  if (/cloud has ended/.test(e))
    return { title: "That computer is no longer available", body: "Cua Cloud has ended. Pick a computer on this Mac instead.", action: "change-computer", retry: false };
  if (/more than one|ambiguous/.test(e))
    return { title: "More than one computer has that name", body: "Pick the one you mean.", action: "change-computer", retry: false };
  if (/not found|no such|unknown space|add_space/.test(e))
    return { title: "Couldn't find that computer", body: "It may have been deleted or renamed. Pick another one.", action: "change-computer", retry: false };
  if (/api key|key|sign in|signed in|auth|login|credential|unauthori[sz]ed|401/.test(e))
    return { title: `${agent} needs your key`, body: `Add it in Settings → Connected apps, then try again.`, action: "add-key", retry: true };
  if (/not running|space_off|stopped|suspended|offline|unreachable|timed out|timeout|connection|refused/.test(e))
    return { title: "Couldn't reach that computer", body: "Check it's on, then try again. Or pick another one.", action: "retry", retry: true };
  if (/disk|space left|no room|memory|quota|limit/.test(e))
    return { title: "This Mac doesn't have room for a new computer", body: "Free some disk space or turn off a computer you're not using, then try again.", action: "retry", retry: true };
  return { title: `${agent} couldn't start`, body: "Something went wrong. Try again, or pick another computer.", action: "retry", retry: true };
}

/** Where a task stands, in the words Home and the thread use. */
export type TaskState = "starting" | "working" | "needs-you" | "stuck" | "done" | "didnt-start" | "failed" | "kept" | "thrown";

/** No new event for this long while a turn runs reads as stuck. */
export const STUCK_MS = 10 * 60_000;

export function taskState(
  task: AskTask,
  run: { status: AgentRunStatus | null; needsYou: boolean; lastEventMs: number | null } | null,
  now = Date.now(),
): TaskState {
  if (task.ending === "thrown") return "thrown";
  if (task.error) return task.runId ? "failed" : "didnt-start";
  if (!task.runId || !run || run.status === null) return task.ending === "kept" ? "kept" : "starting";
  if (run.needsYou) return "needs-you";
  if (run.status === "running") return run.lastEventMs !== null && now - run.lastEventMs > STUCK_MS ? "stuck" : "working";
  if (run.status === "failed" || run.status === "crashed") return "failed";
  return task.ending === "kept" ? "kept" : "done";
}

export const TASK_STATE_TEXT: Record<TaskState, string> = {
  starting: "Getting a computer ready",
  working: "Working",
  "needs-you": "Needs you",
  stuck: "No progress for 10 minutes",
  done: "Done",
  "didnt-start": "Didn't start",
  failed: "Stopped",
  kept: "Done. Computer kept",
  thrown: "Done. Computer thrown away",
};

/** Home's order: needs you first, then running, then the rest, newest first. */
export function orderTasks<T extends { state: TaskState; task: AskTask }>(rows: T[]): T[] {
  const rank = (s: TaskState) => (s === "needs-you" || s === "stuck" ? 0 : s === "working" || s === "starting" ? 1 : 2);
  return [...rows].sort((a, b) => rank(a.state) - rank(b.state) || b.task.createdMs - a.task.createdMs);
}

/** The jobs offered on an empty Home: real dev work where a fresh computer matters. */
export const SUGGESTIONS: { title: string; detail: string; os: SpaceOs; prompt: string }[] = [
  {
    title: "Run my project's tests on a clean Linux computer",
    detail: "Find out if it works on a computer that has never seen your code.",
    os: "linux",
    prompt: "Copy my project onto this computer, install what it needs and run its tests. Tell me what passed and what failed: ",
  },
  {
    title: "Try a pull request before merging",
    detail: "Paste the link. It builds the branch and tells you what breaks.",
    os: "linux",
    prompt: "Check out this pull request, build it and run its tests, then tell me if it's safe to merge: ",
  },
  {
    title: "Check my site works on Safari",
    detail: "It clicks through your pages on a Mac and shows you what looks wrong.",
    os: "macos",
    prompt: "Open my site in Safari, click through the main pages and tell me what looks or works wrong: ",
  },
];
