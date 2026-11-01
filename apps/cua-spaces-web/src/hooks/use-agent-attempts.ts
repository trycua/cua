// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * This device's runs that aren't an agent in a Space (yet), for the one
 * agent list (`agentList.view`): an Ask-first job whose Space is still being
 * made, an @mention of an agent that isn't connected (the thread's note), a
 * start that failed. Only gathers what is on this device; the core decides
 * the rows, their words and their order.
 */

import { useMemo } from "react";

import { useBridge, useSpaces, type AgentChat, type ChatAttempt } from "@/bridge";
import { useStartTask } from "@/components/ask-first/use-start-task";
import { connectNote, useAllWakeNotes } from "@/components/space-thread/summon";
import { useConnectors } from "@/hooks/use-connectors";
import { taskTitle, useAskTasks } from "@/lib/ask-first";
import { agentName } from "@/components/ask-first/ask-box";

const HARNESS: Record<string, string> = { "openai-codex": "codex" };

export function useAgentAttempts(): ChatAttempt[] {
  const tasks = useAskTasks();
  const notes = useAllWakeNotes();
  const { data: spaces } = useSpaces();
  return useMemo(() => {
    const nameOf = (id: string | null) => (id ? (spaces?.find((s) => s.id === id)?.name ?? "") : "");
    const out: ChatAttempt[] = [];
    for (const t of tasks) {
      if (t.inSpace === undefined && !t.pendingId) continue;
      const harness = HARNESS[t.agent] ?? t.agent;
      const base = { harness, label: agentName(t.agent), job: taskTitle(t.prompt), atMs: t.createdMs, note: null, connectLabel: null, app: null };
      if (t.pendingId && !t.spaceId && !t.error) out.push({ ...base, id: `task:${t.id}`, spaceId: t.pendingId, spaceName: nameOf(t.pendingId) || "a new Space", state: "creating" });
      else if (t.error && t.inSpace !== true) out.push({ ...base, id: `task:${t.id}`, spaceId: t.spaceId ?? "", spaceName: nameOf(t.spaceId), state: "failed" });
    }
    for (const [spaceId, list] of Object.entries(notes)) {
      for (const n of list ?? []) {
        if (n.state === "retried") continue;
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
  }, [tasks, notes, spaces]);
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
