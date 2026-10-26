// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useNavigate } from "@tanstack/react-router";

import { newPendingId, useAgents, useBridge, useFeatureRecorder, useFirstSpaceOffers, useSpaces, type HostStatus } from "@/bridge";
import { addAskTask, askTask, HOST_NOT_SHARED, hostSpaceId, retryTarget, taskTarget, updateAskTask, whereOs, type AskTask, type TaskTarget, type TaskWhere } from "@/lib/ask-first";

/**
 * Starts a task: records it, opens its conversation at once, makes the
 * computer (or uses the one picked), then starts the coding app on it with
 * the job as its prompt. This Mac runs it in its own Space id, the one its
 * host service shares it as (`hostSpaceId`). A failure is kept on the task and shown in its
 * thread, which offers Try again (`retry`) and Change computer
 * (`retry(task, where)`).
 */
export function useStartTask(): {
  start: (job: { prompt: string; agent: string; where: TaskWhere }) => void;
  retry: (task: AskTask, where?: TaskWhere) => void;
} {
  const navigate = useNavigate();
  const { createSpace } = useSpaces();
  const { data: host } = useBridge();
  const { startRun } = useAgents();
  const { offers } = useFirstSpaceOffers();
  const record = useFeatureRecorder();

  const run = (id: string, target: TaskTarget) =>
    void (async () => {
      try {
        let spaceId = target.kind === "space" ? target.spaceId : null;
        if (target.kind === "host") {
          const status = (await host?.call("host.status", {} as never).catch(() => null)) as HostStatus | null;
          spaceId = hostSpaceId(status);
          if (!spaceId) throw new Error(HOST_NOT_SHARED);
          updateAskTask(id, { spaceId, onHost: true });
        }
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
      const task = addAskTask({ prompt: prompt.trim(), agent, os: whereOs(where), spaceId: target.kind === "space" ? target.spaceId : null, onHost: target.kind === "host" });
      record("ask_first_task_started");
      void navigate({ to: "/home/$taskId", params: { taskId: task.id } });
      run(task.id, target);
    },
    retry: (task, where) => {
      const target = where ? taskTarget(where) : retryTarget(task);
      const os = where ? whereOs(where) : task.os;
      const spaceId = target.kind === "space" ? target.spaceId : target.kind === "create" && !where ? task.spaceId : null;
      updateAskTask(task.id, { error: null, runId: null, pendingId: null, os, spaceId, onHost: target.kind === "host" });
      record("ask_first_task_retried");
      run(task.id, target);
    },
  };
}
