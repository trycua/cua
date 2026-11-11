// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Home's tasks (CUA-2346):
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
import { t } from "@/i18n";

import { isMachineDesktop, orderSpaces, spaceState } from "./spaces";
import { thisComputerLabel, thisComputerText } from "./host-labels";

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
  /**
   * The Space is an agent home (CUA-2630): the root of this task's tree, where
   * the agent decides where to run. Absent on tasks made before it.
   */
  home?: boolean;
  /** Names the person gave as where to run (@mentions, "on my Mac"), for the thread's first line. */
  named?: string[];
  /** It runs on this Mac itself (its own Space id is in `spaceId`): nothing to throw away. */
  onHost?: boolean;
  /** Started in its Space's thread: the job is the Space's first message, @mentioning the agent. */
  inSpace?: boolean;
  /**
   * A chat picked from "Pick up where you left off" (CUA-3319): the agent's
   * own session on this Mac. Only this reference is kept; the earlier
   * messages are read again from the agent's own store when the task opens,
   * and nothing is posted to a Space thread, so the chat stays private to
   * this device's user. The first message the person sends resumes it.
   */
  imported?: ImportedChat;
  /** The model the person picked (its id), when they did. */
  model?: string;
  /** The reasoning level the person picked, when the model has levels. */
  effort?: string;
  /** The repo the task starts from, when one was chosen. */
  repo?: { url: string; label: string };
  /** The computer Home's "Run something" placed it on (`pool.place`). */
  computer?: TaskComputer;
  /** The computer it was meant for, which couldn't be reached, so it moved to `computer`. */
  movedFrom?: TaskComputer;
}

/** One of your computers, as a task remembers it. */
export interface TaskComputer {
  /** The account's id for it (`machines.list`). */
  id: string;
  name: string;
}

/** Which of the agent's own chats a task continues. */
export interface ImportedChat {
  /** The agent's session id. */
  sessionId: string;
  title: string;
  /** The chat's folder: the run continues there. */
  cwd: string | null;
  lastActiveMs: number;
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
export async function postToSpaceThread(
  data: Pick<DataAdapter, "call" | "mode"> | null | undefined,
  spaceId: string | null | undefined,
  text: string,
): Promise<void> {
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

/** Forgets a task's record on this device (a run that never started). Nothing else is touched: a Space, an agent or its thread stay. */
export function removeAskTask(id: string): void {
  save(load().filter((t) => t.id !== id));
}

/**
 * A new Home task for one of the agent's own chats (Continue in Cua). Nothing
 * runs: the task waits for the person's next message. Picking the same chat
 * again opens the task already made for it.
 */
export function addImportedTask(chat: { agent: string; id: string; title: string; cwd: string | null; lastActiveMs: number }, now = Date.now()): AskTask {
  const existing = load().find((t) => t.imported?.sessionId === chat.id && t.agent === chat.agent && !t.runId);
  if (existing) return existing;
  return addAskTask(
    {
      prompt: "",
      agent: chat.agent,
      os: "macos",
      onHost: true,
      imported: { sessionId: chat.id, title: chat.title, cwd: chat.cwd, lastActiveMs: chat.lastActiveMs },
    },
    now,
  );
}

/** A task's title: what was asked, or the continued chat's own title. */
export function askTaskTitle(t: Pick<AskTask, "prompt" | "imported">): string {
  return t.prompt.trim() ? taskTitle(t.prompt) : (t.imported?.title ?? "");
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
  const line =
    prompt
      .split("\n")
      .map((l) => l.trim())
      .find(Boolean) ?? "";
  return [...line].length <= max
    ? line
    : `${[...line]
        .slice(0, max - 1)
        .join("")
        .trimEnd()}…`;
}

/**
 * Where the job runs, as picked: a new computer of a kind on this Mac, a Space
 * you already have (or one on another of your computers, `on`), or this Mac itself. With the Child Spaces experiment on
 * (agent homes, CUA-2630) the agent may also decide (`agent`: it starts in its
 * own home Space and makes what the job needs) or be told one of your machines
 * (`machine`).
 */
export type TaskWhere =
  | { kind: "new"; os: SpaceOs; on?: NewSpaceOn }
  | { kind: "space"; space: Space }
  | { kind: "this-mac" }
  | { kind: "agent"; /** Continue in the agent's existing Home; absent, a message starts a new Home. */ reuse?: boolean }
  | { kind: "machine"; machine: { id: string; name: string } };

/** Another of your computers a new Space is made on ("New Space on Studio Mac mini"); absent: this Mac. */
export interface NewSpaceOn {
  /** Its relay machine id (`host:<machine>` in the create's `on`). */
  machine: string;
  name: string;
}

/**
 * What `agents.start` is pointed at. The core's `agent_start` only takes a
 * Space id (`local:`, `cloud:`, `direct:`, `relay:`), so this Mac (the
 * `this-mac` row) is first turned into its own Space id: the one its host
 * service shares it as (`hostSpaceId`), the same id `cua mcp` treats as
 * This Mac. The person confirms once that the agent works on their own
 * files and apps.
 */
export type TaskTarget = { kind: "create"; os: SpaceOs; on?: string } | { kind: "space"; spaceId: string } | { kind: "host" };

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
export function hostSpaceId(
  status: Pick<HostStatus, "configured" | "mode" | "machineId" | "directUrl" | "sharing" | "shareDesktop"> | null | undefined,
): string | null {
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
  // An agent home is a Linux Space made for the task (`useStartTask` decides whether it is a home).
  if (where.kind === "agent" || where.kind === "machine") return { kind: "create", os: "linux" };
  if (where.kind === "new") return where.on ? { kind: "create", os: where.os, on: `host:${where.on.machine}` } : { kind: "create", os: where.os };
  if (where.kind === "this-mac" || where.space.id === THIS_MAC_ID) return { kind: "host" };
  return { kind: "space", spaceId: where.space.id };
}

export const whereOs = (where: TaskWhere): SpaceOs =>
  where.kind === "new" ? where.os : where.kind === "space" ? where.space.os : where.kind === "agent" || where.kind === "machine" ? "linux" : "macos";

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

/** Your other computers whose desktops are online to run a job on: never this computer's own (`this-mac` is its row). */
export function otherComputerDesktops(spaces: readonly Space[] | undefined, thisMachineId: string | null): Space[] {
  return (spaces ?? []).filter(
    (s) =>
      isMachineDesktop(s) &&
      s.id !== THIS_MAC_ID &&
      s.status !== "local" &&
      s.id !== `relay:${thisMachineId}` &&
      s.status === "running" &&
      spaceState(s) === "running",
  );
}

/** Where a job runs unless you pick: a new Linux computer on this Mac (or the first kind this Mac can make). */
export function defaultWhere(kinds: readonly NewKind[]): TaskWhere {
  const ok = kinds.filter((k) => !k.blocked);
  const os = ok.find((k) => k.os === "linux")?.os ?? ok[0]?.os ?? "linux";
  return { kind: "new", os };
}

/** The pick if it's still offered, else the default (a picked Space that went away falls back). */
export function currentWhere(picked: TaskWhere | null, kinds: readonly NewKind[], spaces: readonly Space[]): TaskWhere {
  if (picked?.kind === "this-mac" || picked?.kind === "agent" || picked?.kind === "machine") return picked;
  if (picked?.kind === "new" && (picked.on || kinds.some((k) => k.os === picked.os && !k.blocked))) return picked;
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
    title: t("home.confirm.title", { computer: thisComputerText() }),
    description: t("home.confirm.description", { agent }),
  };
}

/**
 * A Space on one of your other computers: that computer's own desktop over the relay, or a Space it provides. It
 * can go away between being picked and the job starting, and its thread lives on the relay, so a job posted to it
 * would still "succeed" and wait there for a computer that is gone. A start asks it first (`spaces.reach`).
 */
export function onAnotherComputer(spaceId: string, spaces: readonly Pick<Space, "id" | "provider" | "host">[] | null | undefined): boolean {
  if (spaceId.startsWith("relay:")) return true;
  const row = spaces?.find((s) => s.id === spaceId);
  return Boolean(row && (row.provider === "relay" || row.host));
}

/** Why a start stops before it hands the job over (`spaces.reach` said no): words `isUnreachableError` reads. */
export function unreachableError(spaceId: string, reach: { listedOffline: boolean; reason: string | null }): string {
  const why = reach.reason ?? (reach.listedOffline ? "its computer is offline" : "it did not answer");
  return `${spaceId} is unreachable: ${why}`;
}

/** A start that failed because its computer (or Space) couldn't be reached: another computer can take the job. */
export function isUnreachableError(raw: string): boolean {
  return /not running|space_off|stopped|suspended|offline|unreachable|timed out|timeout|connection|refused/.test(raw.toLowerCase());
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
  /** No known cause: the card also offers Send a report. */
  report?: boolean;
}

/**
 * A core error from making the computer or starting the agent, in plain
 * words with the one thing to do next. The raw text stays on the task and
 * shows under Details.
 */
export function startErrorView(raw: string, agent: string, onHost = false): StartErrorView {
  const e = raw.toLowerCase();
  if (e.includes("isn't shared"))
    return {
      title: t("home.start_error.not_shared_title", { computer: thisComputerLabel() }),
      body: t("home.start_error.not_shared_body", { computer: thisComputerText() }),
      action: "set-up-host",
      retry: true,
    };
  // A task from before this Mac was turned into its own Space id: trying again now works.
  if (e.includes(`"${THIS_MAC_ID}"`))
    return {
      title: t("home.start_error.cant_start_here_title", { computer: thisComputerText() }),
      body: t("home.start_error.cant_start_here_body", { computer: thisComputerText() }),
      action: "retry",
      retry: true,
    };
  if (/cloud has ended/.test(e))
    return {
      title: t("home.start_error.gone_title"),
      body: t("home.start_error.gone_body", { computer: thisComputerText() }),
      action: "change-computer",
      retry: false,
    };
  if (/more than one|ambiguous/.test(e))
    return { title: t("home.start_error.ambiguous_title"), body: t("home.start_error.ambiguous_body"), action: "change-computer", retry: false };
  if (/not found|no such|unknown space|add_space/.test(e))
    return { title: t("home.start_error.not_found_title"), body: t("home.start_error.not_found_body"), action: "change-computer", retry: false };
  if (/api key|key|sign in|signed in|auth|login|credential|unauthori[sz]ed|401/.test(e))
    return { title: t("home.start_error.key_title", { agent }), body: t("home.start_error.key_body"), action: "add-key", retry: true };
  if (isUnreachableError(e))
    // This computer is plainly on: "Check it's on" would send the person looking for nothing.
    return onHost
      ? {
          title: t("home.start_error.host_unreachable_title", { computer: thisComputerLabel() }),
          body: t("home.start_error.host_unreachable_body", { computer: thisComputerText() }),
          action: "retry",
          retry: true,
        }
      : { title: t("home.start_error.unreachable_title"), body: t("home.start_error.unreachable_body"), action: "retry", retry: true };
  if (/disk|space left|no room|memory|quota|limit/.test(e))
    return {
      title: t("home.start_error.no_room_title", { computer: thisComputerLabel() }),
      body: t("home.start_error.no_room_body"),
      action: "retry",
      retry: true,
    };
  return { title: t("home.start_error.failed_title", { agent }), body: t("home.start_error.failed_body"), action: "retry", retry: true, report: true };
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

/** A task that never got a run (it did not start, or never got past starting) and is older than this is not a task any
 * more: Home clears it itself, so an old failure does not sit on Home for days. */
export const STALE_TASK_MS = 30 * 60_000;

export function withoutStaleTasks<T extends Pick<AskTask, "runId" | "createdMs" | "ending">>(tasks: readonly T[], now = Date.now()): T[] {
  return tasks.filter((t) => t.runId || t.ending || now - t.createdMs < STALE_TASK_MS);
}

/** An object whose values are catalog strings read when used, so a language change shows at once. */
function lazyWords<K extends string>(keys: Record<K, string>): Record<K, string> {
  return Object.defineProperties(
    {},
    Object.fromEntries(Object.entries<string>(keys).map(([k, key]) => [k, { enumerable: true, get: () => t(key) }])),
  ) as Record<K, string>;
}

export const TASK_STATE_TEXT: Record<TaskState, string> = lazyWords<TaskState>({
  starting: "home.state.starting",
  working: "home.state.working",
  "needs-you": "home.state.needs_you",
  stuck: "home.state.stuck",
  done: "home.state.done",
  "didnt-start": "home.state.didnt_start",
  failed: "home.state.failed",
  kept: "home.state.kept",
  thrown: "home.state.thrown",
});

/** The same states for a task in an agent home: its Space is a home, not a computer made for the job. */
export const TASK_STATE_TEXT_HOME: Partial<Record<TaskState, string>> = lazyWords<"starting" | "kept" | "thrown">({
  starting: "home.state.home_starting",
  kept: "home.state.home_kept",
  thrown: "home.state.home_thrown",
});

/** A task's state line. */
export const taskStateText = (state: TaskState, task: Pick<AskTask, "home">): string =>
  (task.home ? TASK_STATE_TEXT_HOME[state] : undefined) ?? TASK_STATE_TEXT[state];

/** Home's order: needs you first, then running, then the rest, newest first. */
export function orderTasks<T extends { state: TaskState; task: AskTask }>(rows: T[]): T[] {
  const rank = (s: TaskState) => (s === "needs-you" || s === "stuck" ? 0 : s === "working" || s === "starting" ? 1 : 2);
  return [...rows].sort((a, b) => rank(a.state) - rank(b.state) || b.task.createdMs - a.task.createdMs);
}

/** The jobs offered on an empty Home: real dev work where a fresh computer matters. */
export const SUGGESTIONS: { title: string; detail?: string; os: SpaceOs; prompt: string }[] = [
  {
    get title() {
      return t("home.suggestion.tests_title");
    },
    os: "linux",
    get prompt() {
      return t("home.suggestion.tests_prompt");
    },
  },
  {
    get title() {
      return t("home.suggestion.pr_title");
    },
    get detail() {
      return t("home.suggestion.pr_detail");
    },
    os: "linux",
    get prompt() {
      return t("home.suggestion.pr_prompt");
    },
  },
  {
    get title() {
      return t("home.suggestion.safari_title");
    },
    os: "macos",
    get prompt() {
      return t("home.suggestion.safari_prompt");
    },
  },
];
