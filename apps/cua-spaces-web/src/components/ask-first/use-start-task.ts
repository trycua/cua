// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useNavigate } from "@tanstack/react-router";

import { newPendingId, useAgents, useFeatureRecorder, useFirstSpaceOffers, useSpaces } from "@/bridge";
import { addAskTask, askTask, HOST_TARGET_ERROR, retryTarget, taskTarget, updateAskTask, type AskTask, type TaskTarget, type TaskWhere } from "@/lib/ask-first";

/**
 * Starts a task: records it, opens its conversation at once, makes the
 * computer (or uses the one picked), then starts the coding app on it with
 * the job as its prompt. A failure is kept on the task and shown in its
 * thread, which offers Try again (`retry`) and Change computer
 * (`retry(task, where)`).
 */
export function useStartTask(): {
  start: (job: { prompt: string; agent: string; where: TaskWhere }) => void;
  retry: (task: AskTask, where?: TaskWhere) => void;
} {
  const navigate = useNavigate();
  const { data: spaces, createSpace } = useSpaces();
  const { startRun } = useAgents();
  const { offers } = useFirstSpaceOffers();
  const record = useFeatureRecorder();

  const run = (id: string, target: TaskTarget) =>
    void (async () => {
      try {
        if (target.kind === "host") throw new Error(HOST_TARGET_ERROR);
        let spaceId = target.kind === "space" ? target.spaceId : null;
        if (!spaceId) {
          const os = target.kind === "create" ? target.os : "linux";
          const pendingId = newPendingId();
          updateAskTask(id, { pendingId });
          const offer = offers.find((o) => o.os === os);
          const space = await createSpace(offer ? { ...offer.args, os } : { os }, pendingId);
          spaceId = space.id;
          updateAskTask(id, { spaceId, pendingId: null });
        }
        const prompt = askTask(id)?.prompt ?? "";
        const started = await startRun({ spaceId, agent: askTask(id)?.agent ?? "", prompt });
        updateAskTask(id, { runId: started.run_id });
      } catch (e) {
        updateAskTask(id, { error: e instanceof Error ? e.message : String(e), pendingId: null });
      }
    })();

  return {
    start: ({ prompt, agent, where }) => {
      const target = taskTarget(where);
      const os = where.kind === "new" ? where.os : where.space.os;
      const task = addAskTask({ prompt: prompt.trim(), agent, os, spaceId: target.kind === "space" ? target.spaceId : null });
      record("ask_first_task_started");
      void navigate({ to: "/home/$taskId", params: { taskId: task.id } });
      run(task.id, target);
    },
    retry: (task, where) => {
      const target = where ? taskTarget(where) : retryTarget(task, spaces);
      const os = where ? (where.kind === "new" ? where.os : where.space.os) : task.os;
      updateAskTask(task.id, { error: null, runId: null, pendingId: null, os, spaceId: target.kind === "space" ? target.spaceId : where ? null : task.spaceId });
      record("ask_first_task_retried");
      run(task.id, target);
    },
  };
}
