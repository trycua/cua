// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useNavigate } from "@tanstack/react-router";

import { newPendingId, useAgents, useFeatureRecorder, useFirstSpaceOffers, useSpaces } from "@/bridge";
import { addAskTask, updateAskTask } from "@/lib/ask-first";

import type { TaskWhere } from "./ask-box";

/**
 * Starts a task: records it, opens its conversation at once, makes the
 * computer (or uses the one picked), then starts the coding app on it with
 * the job as its prompt. A failure is kept on the task and shown in its
 * thread.
 */
export function useStartTask(): (job: { prompt: string; agent: string; where: TaskWhere }) => void {
  const navigate = useNavigate();
  const { createSpace } = useSpaces();
  const { startRun } = useAgents();
  const { offers } = useFirstSpaceOffers();
  const record = useFeatureRecorder();
  return ({ prompt, agent, where }) => {
    const os = where.kind === "new" ? where.os : where.space.os;
    const task = addAskTask({ prompt: prompt.trim(), agent, os, spaceId: where.kind === "space" ? where.space.id : null });
    record("ask_first_task_started");
    void navigate({ to: "/home/$taskId", params: { taskId: task.id } });
    void (async () => {
      try {
        let spaceId = task.spaceId;
        if (!spaceId) {
          const pendingId = newPendingId();
          updateAskTask(task.id, { pendingId });
          const offer = offers.find((o) => o.os === os);
          const space = await createSpace(offer ? { ...offer.args, os } : { os }, pendingId);
          spaceId = space.id;
          updateAskTask(task.id, { spaceId, pendingId: null });
        }
        const run = await startRun({ spaceId, agent, prompt: task.prompt });
        updateAskTask(task.id, { runId: run.run_id });
      } catch (e) {
        updateAskTask(task.id, { error: e instanceof Error ? e.message : String(e) });
      }
    })();
  };
}
