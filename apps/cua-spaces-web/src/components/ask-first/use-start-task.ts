// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useNavigate, useRouter } from "@tanstack/react-router";

import { firstMessage, newPendingId, useAgents, useBridge, useFeatureRecorder, useFirstSpaceOffers, useMachines, useSpaces, type HostStatus } from "@/bridge";
import type { SpaceCreateConfig } from "@/bridge/contracts/host";
import { useChildren } from "@/components/children/use-children";
import { useSummonIn } from "@/components/ask-first/use-summon-in";
import { dropQueue, enqueue, flushQueue, resolveQueue } from "@/components/space-thread/queued";
import { useConnectors } from "@/hooks/use-connectors";
import { containerHome } from "@/lib/agent-home/container";
import { grantsFor, homeGrantsArg, homeMentionsArg } from "@/lib/agent-home/grants";
import { homeForRequest, rememberHome } from "@/lib/agent-home/home-space";
import { petName } from "@/lib/pet-name";
import { hostStatusWhenOnline } from "@/lib/this-mac-target";
import { parseMentions } from "@/lib/agent-home/mentions";
import { homeBrief, homePrompt } from "@/lib/agent-home/prompt";
import type { AskRepo } from "@/lib/ask-first-repo";
import { getDriver } from "@/lib/prepare/store";
import {
  addAskTask,
  askTask,
  HOST_NOT_SHARED,
  hostSpaceId,
  isUnreachableError,
  onAnotherComputer,
  postToSpaceThread,
  retryTarget,
  taskTarget,
  unreachableError,
  updateAskTask,
  whereOs,
  type AskTask,
  type TaskComputer,
  type TaskTarget,
  type TaskWhere,
} from "@/lib/ask-first";

/** The job as the agent is told it: the repo the person picked is a line of it, said once in plain words. */
export const withRepo = (prompt: string, repo?: { url: string } | null) => (repo ? `${prompt.trim()}\n\nRepo: ${repo.url}` : prompt);

/** Task and Space pairs whose thread already has the task's job. */
const posted = new Set<string>();

/** Where a job goes instead when its computer can't be reached: the next computer the pool picks, or null. */
export type MoveJob = (unreachable: readonly string[]) => { where: TaskWhere; computer: TaskComputer } | null;

/** How Home's "Run something" starts a job: on one computer, staying on Home, moving it when that computer can't be reached. */
export interface StartOptions {
  /** Open the job's page (the default); Home's grid stays where it is and shows the job on its computer's card. */
  open?: boolean;
  /** The computer the job is placed on. */
  computer?: TaskComputer;
  /** The computer picked first, which couldn't take it. */
  movedFrom?: TaskComputer;
  /** Called when the start fails because its computer can't be reached. */
  move?: MoveJob;
}

/** Each task's way to move, while its start is in flight. */
const movers = new Map<string, { move: MoveJob; tried: string[] }>();

/**
 * Starts a task: records it, opens its conversation at once, makes the
 * computer (or uses the one picked), then starts the coding app on it with
 * the job as its prompt. This Mac runs it in its own Space id, the one its
 * host service shares it as (`hostSpaceId`). A failure is kept on the task and shown in its
 * thread, which offers Try again (`retry`) and Change computer
 * (`retry(task, where)`).
 *
 * With Child Spaces on and the agent left to decide (or a computer named), the
 * Space is the agent's home (CUA-2630): the slim Linux image, a new one for
 * every request from Home (cheap: it claims a warm Space), its spawn capability
 * on. An existing home is continued only when the person picks it ("Let the agent
 * decide, in your existing Home"), opens it from Your agents or Spaces (its thread
 * posts in place), or retries a task (which keeps the Space it was made in). The agent decides where the work runs,
 * unless the person said where, and the Spaces it makes appear under the home.
 */
export function useStartTask(): {
  start: (job: { prompt: string; agent: string; where: TaskWhere; model?: string; effort?: string; repo?: AskRepo | null }, options?: StartOptions) => void;
  retry: (task: AskTask, where?: TaskWhere) => void;
} {
  const navigate = useNavigate();
  const router = useRouter();
  const { data: spaces, createSpace } = useSpaces();
  const { data: machines } = useMachines();
  const children = useChildren();
  const { data: host } = useBridge();
  const { startRun } = useAgents();
  const { offers } = useFirstSpaceOffers();
  const record = useFeatureRecorder();
  const { core } = useBridge();
  // The job goes straight to its Space: the page is the Space's, and
  // the job is its first message, @mentioning the agent (the same summon as the composer's).
  const { input: connectors } = useConnectors();

  const { summonInput, send, summonIn, canSummon } = useSummonIn();

  /** What the person named in the job: @mentions, "on my Mac", and the picked place (agent homes). */
  const named = (prompt: string, where?: TaskWhere) => {
    const mentions = parseMentions(prompt, {
      spaces: (spaces ?? []).map((s) => ({ id: s.id, name: s.name })),
      machines: (machines ?? []).map((m) => ({ id: m.id, name: m.name, current: m.current })),
    });
    if (where?.kind === "space" && !mentions.spaces.some((s) => s.id === where.space.id)) mentions.spaces.push({ id: where.space.id, name: where.space.name });
    if (where?.kind === "machine" && !mentions.machines.some((m) => m.id === where.machine.id))
      mentions.machines.push({ id: where.machine.id, name: where.machine.name });
    return mentions;
  };

  /** What a home is made with and briefed with: the agent, what it already holds and what the person named. */
  const setupFor = (agent: string, prompt: string, where?: TaskWhere) => ({ agent, grants: grantsFor(agent), mentions: named(prompt, where) });

  /**
   * Throws when a Space on another computer can't take the job now (`spaces.reach`: the relay's word and a health
   * check). Its thread lives on the relay, so a job handed to its agent, or a run started there, would otherwise
   * "succeed" and wait for a computer that is gone; thrown, the start moves the job to the next computer the pool
   * picks. A host that can't ask (an older app) lets the start go on.
   */
  const reachOrThrow = async (spaceId: string) => {
    if (!host || !onAnotherComputer(spaceId, spaces)) return;
    const reach = await host.call("spaces.reach", { spaceId }).catch(() => null);
    if (reach && !reach.reachable) throw new Error(unreachableError(spaceId, reach));
  };

  /** Posts the job to the agent's home: what the person named now is the grant, then the message goes to the agent through the thread. */
  const reuseHome = async (
    spaceId: string,
    agent: string,
    prompt: string,
    model?: string,
    brief?: string,
    effort?: string,
  ): Promise<"posted" | "needs-run"> => {
    // An @mention of a Space or a computer, or "on my Mac", is the approval, so no card asks. It goes to the grants
    // file the daemon reads each time the home asks (no restart).
    const mentions = named(prompt);
    const g = grantsFor(agent);
    if ((mentions.machines.length || mentions.spaces.length) && host) {
      const names = new Map((machines ?? []).map((m) => [m.id, m.name] as const));
      const merged = { listAll: g.listAll, machines: [...new Set([...g.machines, ...mentions.machines.map((m) => m.id)])] };
      await host.call("spaces.setAccess", { spaceId, home: { ...homeGrantsArg(merged, names), mentions: homeMentionsArg(mentions) } }).catch(() => null);
    }
    // The home's agent is a member of the Space, kept on standby with the picked model.
    if (await summonIn(spaceId, agent, prompt, model, true, brief, effort)) return "posted";
    return "needs-run";
  };

  const run = (id: string, target: TaskTarget, place?: { pendingId?: string; queued?: boolean; reuse?: string; onCreated?: (spaceId: string) => void }) =>
    void (async () => {
      try {
        let spaceId = target.kind === "space" ? target.spaceId : null;
        const task0 = askTask(id);
        const isHome = Boolean(task0?.home && children);
        if (target.kind === "host") {
          // Waits out the few seconds its sharing service restarts after Set up (CUA-3974).
          const status = await hostStatusWhenOnline(
            async () => ((await host?.call("host.status", {} as never).catch(() => null)) ?? null) as HostStatus | null,
          );
          spaceId = hostSpaceId(status);
          if (!spaceId) throw new Error(HOST_NOT_SHARED);
          updateAskTask(id, { spaceId, onHost: true });
        }
        // A home the person chose to continue (`place.reuse`); otherwise one is made below.
        if (!spaceId && isHome && place?.reuse) {
          spaceId = place.reuse;
          updateAskTask(id, { spaceId, pendingId: null });
        }
        // A Space that already exists, with an agent in it or not: its computer may have gone since it was picked.
        // Asked before anything is handed to it (a Space made below has just answered).
        if (spaceId && target.kind !== "host") await reachOrThrow(spaceId);
        if (!spaceId) {
          const os = target.kind === "create" ? target.os : "linux";
          const pendingId = place?.pendingId ?? newPendingId();
          updateAskTask(id, { pendingId });
          let space;
          if (isHome && children) {
            // The agent home: a slim Linux Space, the root of this person's tree (the agent decides where the work runs).
            const agent = task0?.agent ?? "";
            const config: SpaceCreateConfig = containerHome.create(
              setupFor(agent, task0?.prompt ?? ""),
              petName((n) => (spaces ?? []).some((x) => x.name === n)),
            );
            space = await createSpace({ ...config, os: "linux" }, pendingId);
            // The spawn capability, whatever the host did with `children` in the config.
            children.setChildrenOn(space.id, true);
            rememberHome(agent, space.id);
          } else {
            const offer = offers.find((o) => o.os === os);
            // "New Space on <computer>": made on that computer (`host:<machine>`), never silently.
            const on = target.kind === "create" && target.on ? { on: target.on } : {};
            space = await createSpace(offer ? { ...offer.args, os, ...on } : { os, ...on }, pendingId);
          }
          spaceId = space.id;
          updateAskTask(id, { spaceId, pendingId: null });
          place?.onCreated?.(spaceId);
          // A home's message was shown while it was made; it is posted below with its briefing.
          if (isHome) resolveQueue(pendingId, spaceId);
          if (place?.queued) {
            // The job (and anything typed meanwhile) waited in the Space's thread: send it now.
            resolveQueue(pendingId, spaceId);
            // The picked model goes with the agent when it joins (adding it before the first message names it).
            const t = askTask(id);
            if (t?.model && host && canSummon()) {
              const first = firstMessage(core, await summonInput(spaceId), t.agent, "");
              if (first?.handle)
                await host
                  .call("thread.agentAdd", {
                    spaceId,
                    name: first.handle,
                    harness: first.harness || t.agent,
                    model: t.model,
                    ...(t.effort ? { effort: t.effort } : {}),
                  })
                  .catch(() => null);
            }
            await flushQueue(pendingId, send(spaceId, await summonInput(spaceId)));
            updateAskTask(id, { inSpace: true });
            return;
          }
        }
        const task = askTask(id);
        const prompt = withRepo(task?.prompt ?? "", task?.repo);
        const agent = task?.agent ?? "";
        let homeText: string | null = null;
        if (isHome) {
          homeText = homePrompt(task?.prompt ?? "", setupFor(agent, task?.prompt ?? ""), containerHome.abilities(), task?.repo ?? undefined);
          // A home takes the job through its member (a new one too): the chosen repo is a line of the message.
          const brief = homeBrief(setupFor(agent, task?.prompt ?? ""), containerHome.abilities());
          const outcome = await reuseHome(spaceId, agent, prompt, task?.model, brief, task?.effort);
          // Posted for real now (or not at all): the pending copy does not stay.
          if (place?.pendingId) dropQueue(place.pendingId);
          if (outcome === "posted") {
            updateAskTask(id, { inSpace: true });
            return;
          }
        }
        if (!isHome && place && (await summonIn(spaceId, agent, prompt, task?.model, false, undefined, task?.effort))) {
          updateAskTask(id, { inSpace: true });
          return;
        }
        // The job is the first line of the Space's thread, so the conversation lives with the Space
        // (once per Space: Try again on the same Space does not post it twice).
        if (!posted.has(`${id}\u0000${spaceId}`)) {
          posted.add(`${id}\u0000${spaceId}`);
          await postToSpaceThread(host, spaceId, prompt);
        }
        const started = await startRun({
          spaceId,
          agent,
          prompt: homeText ?? prompt,
          ...(task?.model ? { model: task.model } : {}),
          ...(task?.effort ? { effort: task.effort } : {}),
        });
        updateAskTask(id, { runId: started.run_id, inSpace: true });
      } catch (e) {
        if (place?.pendingId) dropQueue(place.pendingId);
        const raw = e instanceof Error ? e.message : String(e);
        // Its computer couldn't be reached: the job moves to the next one the pool picks, and the task says from where.
        const mover = movers.get(id);
        const from = askTask(id)?.computer;
        if (mover && from && isUnreachableError(raw)) {
          mover.tried.push(from.id);
          const next = mover.move(mover.tried);
          if (next) {
            const to = taskTarget(next.where);
            updateAskTask(id, {
              error: null,
              runId: null,
              pendingId: null,
              os: whereOs(next.where),
              spaceId: to.kind === "space" ? to.spaceId : null,
              onHost: to.kind === "host",
              computer: next.computer,
              movedFrom: from,
            });
            run(id, to);
            return;
          }
        }
        movers.delete(id);
        updateAskTask(id, { error: raw, pendingId: null });
      }
    })();

  return {
    start: ({ prompt, agent, where, model, effort, repo }, options = {}) => {
      const target = taskTarget(where);
      // With Child Spaces on, the agent decides (or is told a computer): its home Space takes the request.
      const home = Boolean(children) && (where.kind === "agent" || where.kind === "machine");
      const mentions = named(prompt, where);
      const reuse = home ? homeForRequest(where, agent, spaces, children?.homesOf(agent)) : null;
      // The first run made a Linux Space for this agent while the person was in the first run: the job is
      // kept by the preparing driver, which sends it exactly once when the agent is ready, even across a quit.
      const preparing = getDriver();
      const prep = preparing?.state;
      if (!home && preparing && prep?.wanted && prep.harness === agent && target.kind === "create" && target.os === "linux" && !target.on) {
        const spaceId = prep.homeSpaceId;
        const task = addAskTask({
          prompt: prompt.trim(),
          agent,
          os: "linux",
          spaceId,
          pendingId: spaceId ? null : prep.pendingId,
          onHost: false,
          ...(model ? { model } : {}),
          ...(repo ? { repo } : {}),
        });
        record("ask_first_task_started");
        preparing.send({ id: task.id, text: withRepo(prompt, repo), agent, model: model ?? null, repo: repo?.url ?? null });
        void navigate({ to: "/spaces/$spaceId", params: { spaceId: spaceId ?? prep.pendingId! } });
        return;
      }
      const task = addAskTask({
        prompt: prompt.trim(),
        agent,
        os: whereOs(where),
        spaceId: target.kind === "space" ? target.spaceId : null,
        onHost: target.kind === "host",
        ...(home ? { home, named: [...mentions.spaces.map((s) => s.name), ...mentions.machines.map((m) => m.name)] } : {}),
        ...(model ? { model } : {}),
        ...(effort ? { effort } : {}),
        ...(repo ? { repo } : {}),
        ...(options.computer ? { computer: options.computer } : {}),
        ...(options.movedFrom ? { movedFrom: options.movedFrom } : {}),
      });
      record("ask_first_task_started");
      if (options.move) movers.set(task.id, { move: options.move, tried: options.movedFrom ? [options.movedFrom.id] : [] });
      // Home's grid: the job starts where it is and shows on its computer's card; its page is one click away.
      if (options.open === false) {
        run(task.id, target, target.kind === "host" ? undefined : {});
        return;
      }
      // Straight to the Space: the one picked or the one being made (its page follows the create); this Mac opens the task's page.
      const go = (spaceId: string) => void navigate({ to: "/spaces/$spaceId", params: { spaceId } });
      if (target.kind === "space") {
        run(task.id, target, {});
        go(target.spaceId);
      } else if (target.kind === "create") {
        if (reuse) {
          run(task.id, target, { reuse });
          go(reuse);
          return;
        }
        const pendingId = newPendingId();
        // Still on the row being made when it's done (a create quicker than the page): its Space's own page.
        const onCreated = (spaceId: string) => {
          if (decodeURIComponent(router.state.location.pathname) === `/spaces/${pendingId}`)
            void navigate({ to: "/spaces/$spaceId", params: { spaceId }, replace: true });
        };
        // The job shows in the Space's thread at once, queued until the Space is ready (a home gets it with its briefing instead).
        const first = !home && canSummon() ? firstMessage(core, { members: [], people: [], canAdd: true, connectors }, agent, withRepo(prompt, repo)) : null;
        if (first?.handle) enqueue(pendingId, first.text);
        // A new home is made first: the message shows in its thread right away, pending until the home can take it.
        else if (home && canSummon()) {
          const shown = firstMessage(core, { members: [], people: [], canAdd: true, connectors }, agent, withRepo(prompt, repo));
          if (shown?.handle) enqueue(pendingId, shown.text);
        }
        run(task.id, target, { pendingId, onCreated, queued: Boolean(first?.handle) });
        go(pendingId);
      } else {
        // This Mac's own Space id (`relay:<machine>`) is not in the Spaces list: the roster
        // swaps it for the `this-mac` row, so `/spaces/relay:<machine>` says "This Space isn't
        // here" (trycua/cua#6363). The task's page is where this Mac's conversation lives, and
        // it also says why when the start fails. No `place`: nothing summons in a Space page
        // the person can't open; the run starts straight from the task.
        run(task.id, target);
        void navigate({ to: "/home/$taskId", params: { taskId: task.id } });
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
