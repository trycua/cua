// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * This device's runs that aren't an agent in a Space (yet), for the one
 * agent list (`agentList.view`): an Ask-first job whose Space is still being
 * made, an @mention of an agent that isn't connected (the thread's note), a
 * start that failed. Only gathers what is on this device; the core decides
 * the rows, their words and their order.
 */

import { useEffect, useMemo, useState } from "react";

import { useBridge, useSpaces, type AgentChat, type ChatAttempt } from "@/bridge";
import { useStartTask } from "@/components/ask-first/use-start-task";
import { connectNote, dismissWakeNote, useAllWakeNotes, type WakeNote } from "@/components/space-thread/summon";
import { useConnectors } from "@/hooks/use-connectors";
import { removeAskTask, taskTitle, useAskTasks, withoutStaleTasks, type AskTask } from "@/lib/ask-first";
import { agentName } from "@/components/ask-first/ask-box";

const HARNESS: Record<string, string> = { "openai-codex": "codex" };

/** A job whose Space has no row at all (no pending create, no Space) this long after it was asked is not being made. */
export const CREATE_ROW_GRACE_MS = 2 * 60_000;
/** Even with a row, a Space nobody finished making in this long is not coming (a first macOS download takes well under it). */
export const CREATE_MAX_MS = 90 * 60_000;

/**
 * Whether an Ask-first job still waits for its Space: it has a pending create that is not failed, and either the
 * create's row is in the list or the job is new. A job whose create left no row (the host dropped it, the app quit
 * mid-create, a child Space an agent asked for that never started) must not read "Waiting for a new Space" forever.
 */
export function stillCreating(task: { createdMs: number; pendingId: string | null }, hasRow: boolean, now: number): boolean {
  const age = now - task.createdMs;
  if (age > CREATE_MAX_MS) return false;
  return hasRow || age <= CREATE_ROW_GRACE_MS;
}

/**
 * A run that never started and is gone from the list for good ("orphaned"): its record is removed, nothing else.
 * - Its Space is not there any more: it names a real Space id (not a `pending:` one) that is missing from a Spaces
 *   list that has loaded and is not empty, and it is older than `CREATE_ROW_GRACE_MS` (a Space just made may not be
 *   listed yet). An empty or unread list never removes anything.
 * - Or it is older than `ORPHAN_MAX_MS`: a run that did not start in a week is not going to.
 * A run that has started (`runId`), one being connected right now, and an agent's thread or data are never touched.
 */
export const ORPHAN_MAX_MS = 7 * 24 * 60 * 60_000;

export interface Orphans {
  taskIds: string[];
  notes: { spaceId: string; id: string }[];
}

export function orphanedRuns(
  tasks: readonly AskTask[],
  notes: Record<string, WakeNote[] | undefined>,
  spaces: readonly { id: string }[] | undefined,
  now: number,
): Orphans {
  const known = spaces && spaces.length > 0 ? new Set(spaces.map((s) => s.id)) : null;
  const gone = (id: string | null | undefined, atMs: number | undefined) =>
    Boolean(known && id && !id.startsWith("pending:") && !known.has(id) && atMs !== undefined && now - atMs > CREATE_ROW_GRACE_MS);
  const old = (atMs: number | undefined) => atMs !== undefined && atMs > 0 && now - atMs > ORPHAN_MAX_MS;
  const out: Orphans = { taskIds: [], notes: [] };
  for (const t of tasks) {
    // Only a run that never started: no run, not ended, and listed as an attempt.
    if (t.runId || t.ending || (t.inSpace === undefined && !t.pendingId)) continue;
    if (old(t.createdMs) || (t.error && gone(t.spaceId, t.createdMs))) out.taskIds.push(t.id);
  }
  for (const [spaceId, list] of Object.entries(notes)) {
    for (const n of list ?? []) {
      if (n.state === "connecting") continue;
      if (old(n.atMs) || gone(spaceId, n.atMs ?? 0)) out.notes.push({ spaceId, id: n.id });
    }
  }
  return out;
}

export function useAgentAttempts(): ChatAttempt[] {
  const allTasks = useAskTasks();
  // A job that never got a run and is half an hour old is not an attempt any more: it clears itself.
  const tasks = useMemo(() => withoutStaleTasks(allTasks), [allTasks]);
  const notes = useAllWakeNotes();
  const { data: spaces } = useSpaces();
  // Time passes for a job that waits: the list is judged again every 15 s.
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 15_000);
    return () => clearInterval(t);
  }, []);
  // Orphaned runs leave the list by themselves (their records only).
  useEffect(() => {
    const o = orphanedRuns(allTasks, notes, spaces, now);
    for (const id of o.taskIds) removeAskTask(id);
    for (const n of o.notes) dismissWakeNote(n.spaceId, n.id);
  }, [allTasks, notes, spaces, now]);
  return useMemo(() => {
    const nameOf = (id: string | null) => (id ? (spaces?.find((s) => s.id === id)?.name ?? "") : "");
    const out: ChatAttempt[] = [];
    for (const t of tasks) {
      if (t.inSpace === undefined && !t.pendingId) continue;
      const harness = HARNESS[t.agent] ?? t.agent;
      const base = { harness, label: agentName(t.agent), job: taskTitle(t.prompt), atMs: t.createdMs, note: null, connectLabel: null, app: null };
      const waiting = Boolean(t.pendingId && !t.spaceId && !t.error);
      if (waiting && !stillCreating(t, Boolean(spaces?.some((s) => s.id === t.pendingId)), now))
        out.push({ ...base, id: `task:${t.id}`, spaceId: "", spaceName: "", state: "failed" });
      else if (waiting)
        out.push({ ...base, id: `task:${t.id}`, spaceId: t.pendingId ?? "", spaceName: nameOf(t.pendingId) || "a new Space", state: "creating" });
      else if (t.error && t.inSpace !== true)
        out.push({ ...base, id: `task:${t.id}`, spaceId: t.spaceId ?? "", spaceName: nameOf(t.spaceId), state: "failed" });
    }
    for (const [spaceId, list] of Object.entries(notes)) {
      for (const n of list ?? []) {
        // An @ of nobody here is a note under its message only: it is no agent's run.
        if (n.state === "retried" || n.cant.reason === "no-such-member") continue;
        out.push({
          id: `note:${spaceId}:${n.id}`,
          spaceId,
          spaceName: nameOf(spaceId),
          harness: n.cant.harness,
          label: n.cant.name,
          job: taskTitle(n.text.replace(/^@[\w.-]+\s+/, "")),
          atMs: n.atMs ?? 0,
          state: n.cant.reason === "not-connected" ? "not-connected" : "failed",
          note: n.cant.text,
          connectLabel: n.cant.connectLabel,
          app: n.cant.app,
        });
      }
    }
    return out;
  }, [tasks, notes, spaces, now]);
}

/** Connect or Try again on a run in the list (Home, the Agents page): the same steps as in its thread. */
export function useAttemptAction(): (chat: AgentChat) => void {
  const { data } = useBridge();
  const c = useConnectors();
  const notes = useAllWakeNotes();
  const tasks = useAskTasks();
  const { retry } = useStartTask();
  return (chat) => {
    const id = chat.attempt ?? "";
    if (id.startsWith("note:") && data) {
      for (const [spaceId, list] of Object.entries(notes)) {
        const note = list?.find((n) => `note:${spaceId}:${n.id}` === id);
        if (note) return void connectNote(spaceId, note, c, data);
      }
    }
    if (id.startsWith("task:")) {
      const task = tasks.find((t) => `task:${t.id}` === id);
      if (task) retry(task);
    }
  };
}

/** Remove (the X on a run that hasn't started): its record goes from this device, nothing else. */
export function removeAttempt(chat: Pick<AgentChat, "attempt">, notes: Record<string, WakeNote[] | undefined>): void {
  const id = chat.attempt ?? "";
  if (id.startsWith("task:")) return removeAskTask(id.slice("task:".length));
  if (id.startsWith("note:"))
    for (const [spaceId, list] of Object.entries(notes)) {
      const note = list?.find((n) => `note:${spaceId}:${n.id}` === id);
      if (note) return dismissWakeNote(spaceId, note.id);
    }
}

/** The Remove and Clear all of the Agents list's Not started section. */
export function useAttemptRemoval(): { remove: (chat: AgentChat) => void; clear: (chats: readonly AgentChat[]) => void } {
  const notes = useAllWakeNotes();
  return {
    remove: (chat) => removeAttempt(chat, notes),
    clear: (chats) => {
      for (const c of chats) if (c.kind === "attempt") removeAttempt(c, notes);
    },
  };
}
