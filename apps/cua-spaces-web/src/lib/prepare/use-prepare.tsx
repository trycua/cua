// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect, useRef } from "react";

import { firstMessage, newPendingId, useBridge, useFirstSpaceOffers, useSavedOnboarding, useSession, useSpaces } from "@/bridge";
import { useInstalledAgents } from "@/components/ask-first/use-installed-agents";
import { useSummonIn } from "@/components/ask-first/use-summon-in";
import { setPrepareSink } from "@/components/space-thread/queued";
import { updateAskTask } from "@/lib/ask-first";
import { spaceState } from "@/lib/spaces";

import { localPrepareStorage, PrepareDriver, type PrepareEnv } from "./driver";
import { homeCreateRequest } from "./home-request";
import { getDriver, homeNameOf, markResumed, resumedThisLoad, setDriver, usePrepare, warmStartHeld } from "./store";

/** The coding app the first Space is for when it is on this computer: the first installed one otherwise. */
export const DEFAULT_HARNESS = "claude-code";

/** How long the agent's session may take to open before preparing says so. */
const SESSION_TIMEOUT_MS = 4 * 60_000;
/** How long a create the host started before this page loaded may take (a slow first run). */
const CREATE_FOLLOW_MS = 40 * 60_000;
/** The name of the Space this prepares: fixed, so a second page load finds the first one's create. */
export { homeNameOf } from "./store";
const POLL_MS = 800;

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

/**
 * Mounted once at the root: keeps the driver's view of the host current, and warms the home
 * Home's prompt box writes into (Warm start, always on).
 *
 * The home is the Space the daemon keeps ready: once the first run is over (done or skipped)
 * the app claims it with a create under the fixed name, resumes a saved record once per page
 * load, rebuilds a removed home and follows the connection. It never shows: no list, count,
 * the tree or the Machines page lists it until a person writes into it (`useHiddenSpaceIds`).
 * Nothing starts while the first run shows, and nothing starts for a computer with no coding
 * app installed.
 */
export function PrepareRoot(): null {
  const { core, data: host } = useBridge();
  const spacesHook = useSpaces();
  const { data: spaces, createSpace, startSpace } = spacesHook;
  const { offers, known } = useFirstSpaceOffers();
  const latestOffers = useRef(offers);
  latestOffers.current = offers;
  // The new first run makes nothing the person didn't ask for: the home starts warming only once the first
  // run is done or skipped, not while it shows.
  const session = useSession();
  const savedOnboarding = useSavedOnboarding();
  const firstRunOver = Boolean(session.data?.onboarding.completed || savedOnboarding.skipped);
  // Only an agent found on this computer is prepared: one Cua only wrote files for has no sign-in to bring.
  const installed = useInstalledAgents();
  const harness = installed === null ? null : installed.has(DEFAULT_HARNESS) ? DEFAULT_HARNESS : (installed.values().next().value ?? null);
  const { summonInput, summonIn, canSummon } = useSummonIn();
  const latestSpaces = useRef(spaces);
  latestSpaces.current = spaces;

  const env: PrepareEnv = {
    now: () => Date.now(),
    online: () => globalThis.navigator?.onLine !== false,
    newPendingId,
    findHome: (harness) => {
      // The Space this prepares has a fixed name, so a create the host is still running (the app was quit
      // meanwhile) is found by it.
      const row = spaces?.find((s) => s.name === homeNameOf(harness) && spaceState(s) !== "failed");
      if (!row) return null;
      const state = spaceState(row);
      return { id: row.id, running: state === "running", creating: state === "creating" };
    },
    followCreate: async (id) => {
      const begun = Date.now();
      const until = begun + CREATE_FOLLOW_MS;
      for (;;) {
        const row = latestSpaces.current?.find((s) => s.id === id);
        const state = row ? spaceState(row) : null;
        if (state === "running") return;
        if (state === "failed") throw new Error(row?.progress?.error ?? "Making the home failed");
        if (!row && Date.now() - begun > 15_000) throw new Error("The home is gone");
        if (Date.now() > until) throw new Error("Making the home took too long");
        await sleep(POLL_MS);
      }
    },
    startHome: async (id) => {
      await startSpace(id);
    },
    createHome: async ({ harness: forApp, pendingId }) => {
      // The same Linux Space Home makes for a job (the wizard's own offer for this machine), under the fixed name.
      const space = await createSpace(homeCreateRequest(latestOffers.current, homeNameOf(forApp)), pendingId);
      return space.id;
    },
    attachAgent: async ({ spaceId, harness, model }) => {
      if (!host || !canSummon()) throw new Error("This app can't put an agent in a Space");
      const input = await summonInput(spaceId);
      const first = firstMessage(core, input, harness, "hello");
      if (!first?.handle) throw new Error("This agent can't join a Space");
      void model;
      await host.call("thread.agentAdd", { spaceId, name: first.handle, harness: first.harness || harness });
      const until = Date.now() + SESSION_TIMEOUT_MS;
      for (;;) {
        const { members } = await host.call("thread.members", { spaceId });
        const me = members.find((m) => m.harness === (first.harness || harness));
        if (me?.status === "failed") throw new Error(me.failure?.message ?? "The agent didn't start");
        // A member that joined is ready for its first message: its session opens with it.
        if (me && me.status !== "starting") return;
        if (Date.now() > until) throw new Error("The agent took too long to start");
        await sleep(POLL_MS);
      }
    },
    post: async ({ spaceId, message }) => {
      const ok = await summonIn(spaceId, message.agent, message.text, message.model ?? undefined);
      if (!ok) throw new Error("This agent can't join the Space");
    },
    threadHas: async ({ spaceId, message }) => {
      if (!host) throw new Error("no host");
      const page = await host.call("thread.read", { spaceId, limit: 50 });
      const since = (message.claimedMs ?? message.sentMs) - 2_000;
      return page.entries.some((e) => e.kind === "message" && e.author.kind === "person" && e.tsMs >= since && (e.text ?? "").includes(message.text));
    },
    agentReplied: async ({ spaceId, sinceMs }) => {
      if (!host) return false;
      const page = await host.call("thread.read", { spaceId, limit: 20 });
      return (
        page.live.agents.some((a) => (a.text ?? "").length > 0) ||
        page.entries.some((e) => e.author.kind === "agent" && e.kind === "message" && e.tsMs >= sinceMs)
      );
    },
    homeExists: (id) => (spaces ? spaces.some((s) => s.id === id && s.status !== "deleting") : null),
    delivered: (m, spaceId) => updateAskTask(m.id, { spaceId, pendingId: null, inSpace: true }),
  };

  // One driver for the page; it uses the latest closures.
  getDriver()?.setEnv(env);
  useEffect(() => {
    if (core?.status === "ready" && !getDriver()) setDriver(new PrepareDriver(core, env, localPrepareStorage()));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [core]);

  // The offers must be read first: before that there is no image to make the home from.
  const ready = Boolean(core && host && spaces && known);

  // Start at launch; a saved record resumes once.
  const driverNow = usePrepare().driver;
  useEffect(() => {
    const d = driverNow;
    if (!ready || !d || warmStartHeld()) return;
    if (d.state?.wanted) {
      // A saved record resumes once per page load, however often the layout mounts.
      if (!resumedThisLoad()) {
        markResumed();
        d.resume();
      }
    } else if (harness && firstRunOver) {
      markResumed();
      d.start(harness);
    }
  }, [ready, driverNow, harness, firstRunOver]);

  // The home's composer sends here while it is being made: kept, then delivered once.
  useEffect(() => {
    if (!ready) return;
    setPrepareSink({
      owns: (id) => Boolean(getDriver()?.state?.wanted && (getDriver()?.state?.pendingId === id || getDriver()?.state?.homeSpaceId === id)),
      send: (text) => {
        const d = getDriver();
        if (!d?.state) return;
        d.send({ id: `m-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 6)}`, text, agent: d.state.harness });
      },
    });
    return () => setPrepareSink(null);
  }, [ready]);

  // The create's progress feeds the steps.
  useEffect(() => {
    if (!ready || !host) return;
    return host.subscribe((e) => {
      if (e.type !== "spaces.createProgress" || e.progress.retry) return;
      const d = getDriver();
      // This page's own create, or the one the host kept running for the home when the app was quit.
      if (!d?.state?.wanted || (d.state.pendingId !== e.progress.pendingId && e.progress.space !== `local:${homeNameOf(d.state.harness)}`)) return;
      d.progress({
        phase: e.progress.phase,
        detail: e.progress.detail,
        bytesDone: e.progress.bytesDone,
        bytesTotal: e.progress.bytesTotal,
        bytesPerSecond: e.progress.bytesPerSecond,
      });
    });
  }, [ready, host]);

  // The connection (the browser says when it goes and comes back).
  useEffect(() => {
    if (!ready) return;
    const on = () => getDriver()?.online(true);
    const off = () => getDriver()?.online(false);
    globalThis.addEventListener?.("online", on);
    globalThis.addEventListener?.("offline", off);
    return () => {
      globalThis.removeEventListener?.("online", on);
      globalThis.removeEventListener?.("offline", off);
    };
  }, [ready]);

  // The list changed, so a home that was removed is made again; a second a tick while it waits.
  useEffect(() => {
    if (ready) getDriver()?.checkHome();
  }, [ready, spaces]);
  useEffect(() => {
    if (!ready) return;
    const t = setInterval(() => getDriver()?.tick(), 1_000);
    return () => clearInterval(t);
  }, [ready]);

  return null;
}
