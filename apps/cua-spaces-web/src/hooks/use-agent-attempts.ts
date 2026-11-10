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
import { connectNote, useAllWakeNotes } from "@/components/space-thread/summon";
import { useConnectors } from "@/hooks/use-connectors";
import { taskTitle, useAskTasks, withoutStaleTasks } from "@/lib/ask-first";
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
