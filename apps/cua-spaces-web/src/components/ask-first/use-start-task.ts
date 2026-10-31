// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useNavigate, useRouter } from "@tanstack/react-router";

import { firstMessage, newPendingId, threadOpAvailable, useAgents, useBridge, useFeatureRecorder, useFirstSpaceOffers, useSpaces, type AgentMember, type HostStatus } from "@/bridge";
import { enqueue, flushQueue, resolveQueue } from "@/components/space-thread/queued";
import { sendWithSummon } from "@/components/space-thread/summon";
import { useConnectors } from "@/hooks/use-connectors";
import { addAskTask, askTask, HOST_NOT_SHARED, hostSpaceId, postToSpaceThread, retryTarget, taskTarget, updateAskTask, whereOs, type AskTask, type TaskTarget, type TaskWhere } from "@/lib/ask-first";

/** Task and Space pairs whose thread already has the task's job. */
const posted = new Set<string>();

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
  const router = useRouter();
  const { createSpace } = useSpaces();
  const { data: host } = useBridge();
  const { startRun } = useAgents();
  const { offers } = useFirstSpaceOffers();
  const record = useFeatureRecorder();
  const { core } = useBridge();
  // The job goes straight to its Space: the page is the Space's, and
  // the job is its first message, @mentioning the agent (the same summon as the composer's).
  const { input: connectors } = useConnectors();

  /** What summoning reads for `spaceId`: its agents now and this device's connectors. */
  const summonInput = async (spaceId: string) => {
    const members = ((await host!.call("thread.members", { spaceId }).catch(() => null))?.members ?? []) as AgentMember[];
    return { members: members.map((m) => ({ handle: m.name, harness: m.harness, label: m.name })), people: [], canAdd: true, connectors };
  };
  const send = (spaceId: string, input: Awaited<ReturnType<typeof summonInput>>) => (text: string) =>
    sendWithSummon({ core, data: host!, spaceId, input, text, post: (t) => host!.call("thread.post", { spaceId, text: t }) });
  const canSummon = () => !!host && threadOpAvailable(host.mode, "thread.agentAdd");

  /** The job as the Space's first message, through the summon; false when this agent can't join a Space. */
  const summonIn = async (spaceId: string, agent: string, prompt: string): Promise<boolean> => {
    if (!canSummon()) return false;
    const input = await summonInput(spaceId);
    const first = firstMessage(core, input, agent, prompt);
    if (!first?.handle) return false;
    await send(spaceId, input)(first.text);
    return true;
  };

  const run = (id: string, target: TaskTarget, place?: { pendingId?: string; queued?: boolean; onSpace?: (spaceId: string) => void; onCreated?: (spaceId: string) => void; onNoSpace?: () => void }) =>
    void (async () => {
      try {
        let spaceId = target.kind === "space" ? target.spaceId : null;
        if (target.kind === "host") {
          const status = (await host?.call("host.status", {} as never).catch(() => null)) as HostStatus | null;
          spaceId = hostSpaceId(status);
          if (!spaceId) throw new Error(HOST_NOT_SHARED);
          updateAskTask(id, { spaceId, onHost: true });
          place?.onSpace?.(spaceId);
        }
        if (!spaceId) {
          const os = target.kind === "create" ? target.os : "linux";
          const pendingId = place?.pendingId ?? newPendingId();
          updateAskTask(id, { pendingId });
          const offer = offers.find((o) => o.os === os);
          const space = await createSpace(offer ? { ...offer.args, os } : { os }, pendingId);
          spaceId = space.id;
          updateAskTask(id, { spaceId, pendingId: null });
          place?.onCreated?.(spaceId);
          if (place?.queued) {
            // The job (and anything typed meanwhile) waited in the Space's thread: send it now.
            resolveQueue(pendingId, spaceId);
            await flushQueue(pendingId, send(spaceId, await summonInput(spaceId)));
            updateAskTask(id, { inSpace: true });
            return;
          }
        }
        const prompt = askTask(id)?.prompt ?? "";
        if (place && (await summonIn(spaceId, askTask(id)?.agent ?? "", prompt))) {
          updateAskTask(id, { inSpace: true });
          return;
        }
        // The job is the first line of the Space's thread, so the conversation lives with the Space
        // (once per Space: Try again on the same Space does not post it twice).
        if (!posted.has(`${id}\u0000${spaceId}`)) {
          posted.add(`${id}\u0000${spaceId}`);
          await postToSpaceThread(host, spaceId, prompt);
        }
        const started = await startRun({ spaceId, agent: askTask(id)?.agent ?? "", prompt });
        updateAskTask(id, { runId: started.run_id });
      } catch (e) {
        updateAskTask(id, { error: e instanceof Error ? e.message : String(e), pendingId: null });
        // No Space to show it in (this Mac isn't shared): its conversation page says why.
        if (!askTask(id)?.spaceId) place?.onNoSpace?.();
      }
    })();

  return {
    start: ({ prompt, agent, where }) => {
      const target = taskTarget(where);
      const task = addAskTask({ prompt: prompt.trim(), agent, os: whereOs(where), spaceId: target.kind === "space" ? target.spaceId : null, onHost: target.kind === "host" });
      record("ask_first_task_started");
      // Straight to the Space: the one picked, the one being made (its page follows the create), or this Mac's.
      const go = (spaceId: string) => void navigate({ to: "/spaces/$spaceId", params: { spaceId } });
      if (target.kind === "space") {
        run(task.id, target, {});
        go(target.spaceId);
      } else if (target.kind === "create") {
        const pendingId = newPendingId();
        // Still on the row being made when it's done (a create quicker than the page): its Space's own page.
        const onCreated = (spaceId: string) => {
          if (decodeURIComponent(router.state.location.pathname) === `/spaces/${pendingId}`) void navigate({ to: "/spaces/$spaceId", params: { spaceId }, replace: true });
        };
        // The job shows in the Space's thread at once, queued until the Space is ready.
        const first = canSummon() ? firstMessage(core, { members: [], people: [], canAdd: true, connectors }, agent, prompt) : null;
        if (first?.handle) enqueue(pendingId, first.text);
        run(task.id, target, { pendingId, onCreated, queued: Boolean(first?.handle) });
        go(pendingId);
      } else {
        run(task.id, target, { onSpace: go, onNoSpace: () => void navigate({ to: "/home/$taskId", params: { taskId: task.id } }) });
      }
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
