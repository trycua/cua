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

import type { AgentRunStatus, SpaceOs, TranscriptItem } from "@/bridge";

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

/** Where a task stands, in the words Home and the thread use. */
export type TaskState = "starting" | "working" | "needs-you" | "stuck" | "done" | "failed" | "kept" | "thrown";

/** No new event for this long while a turn runs reads as stuck. */
export const STUCK_MS = 10 * 60_000;

export function taskState(
  task: AskTask,
  run: { status: AgentRunStatus | null; needsYou: boolean; lastEventMs: number | null } | null,
  now = Date.now(),
): TaskState {
  if (task.ending === "thrown") return "thrown";
  if (task.error) return "failed";
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
