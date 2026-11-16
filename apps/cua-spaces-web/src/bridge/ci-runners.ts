// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The CI Runners page: GitHub Actions jobs on your own computers. Every
 * sentence, format, tab and step is the app core's (`ciRunners.view`,
 * `ciRunners.reduce`, `cua-spaces-app-core/src/ci_runners.rs`); this module
 * keeps the page's state, reads `cua ci overview` through the host
 * (`ops/ci.ts`) and runs what the core asks for: opening GitHub, asking
 * the connect step again while the person is on GitHub, a token connect, a
 * repository switch, the goal, a job's log. Without the core there is no
 * page (the app core wasm loads everywhere the app runs).
 */

import { useCallback, useEffect, useRef, useState } from "react";

import { isUnsupported } from "./adapter";
import type { CoreClient } from "./core";
import type { CiAction, CiConnectAnswer, CiGoal, CiLogLine, CiOverview, CiState, CiView } from "./contracts/ci-runners";
import { useBridge, useSession } from "./hooks";

/** The page's first state (the core's). */
export function ciInitial(core: CoreClient): CiState | null {
  return core.tryCall<CiState>("ciRunners.reduce", { action: { type: "tab", tab: "all" } }) ?? null;
}

/** The state after `action` (no state: the first one). Without the core, unchanged. */
export function ciReduce(core: CoreClient, state: CiState | undefined, action: CiAction): CiState | undefined {
  return core.tryCall<CiState>("ciRunners.reduce", { state, action }) ?? state;
}

/** The page the core draws for this overview and state. */
export function ciView(core: CoreClient, overview: CiOverview, state: CiState | undefined): CiView | null {
  return core.tryCall<CiView>("ciRunners.view", { overview, state }) ?? null;
}

/** How often the page reads again (sooner while a job is queued or running) and how often the
 * connect step asks again while the person is on GitHub. An object so a test can speed it up. */
export const ciTiming = { pollMs: 30_000, busyPollMs: 5_000, connectPollMs: 2_500 };

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

export interface CiLog {
  lines: CiLogLine[] | null;
  error: string | null;
}

export interface CiRunnersHook {
  /** The page as the core draws it; null until the first read (or without the core). */
  view: CiView | null;
  /** The host cannot run `cua ci` (no bundled cua here). */
  unsupported: boolean;
  /** The app core did not load, so there is no page to draw. */
  noCore: boolean;
  /** The overview could not be read, in a sentence. */
  error: string | null;
  /** A job's log by `job_key`, once asked for. */
  logs: Record<string, CiLog>;
  /** Sends a page action. `token` goes with `tokenSubmit` only: it is never kept. */
  act(action: CiAction, extra?: { token?: string }): void;
  /** A repository's switch: may run jobs or not. */
  setRepository(name: string, allowed: boolean): void;
  /** The cloud's goal. */
  setGoal(target: string, goal: CiGoal): void;
  /** Reads a job's own log. */
  showLog(ref: { jobKey: string; pool: string; slot: string }): void;
  /** Opens a link in the browser. */
  open(url: string): void;
  /** What a switch or the goal just said, once, for a toast. */
  notice: string | null;
}

/** The CI Runners page. */
export function useCiRunners(): CiRunnersHook {
  const { core, data } = useBridge();
  const session = useSession();
  const [overview, setOverview] = useState<CiOverview | null>(null);
  const [state, setState] = useState<CiState | null>(() => ciInitial(core));
  const [unsupported, setUnsupported] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [logs, setLogs] = useState<Record<string, CiLog>>({});
  const [notice, setNotice] = useState<string | null>(null);
  // The app core loads after the first render in a browser: the first state waits for it.
  useEffect(() => {
    if (state) return;
    const first = ciInitial(core);
    if (first) setState(first);
  }, [core, core.status, state]);
  const current = useRef(state);
  current.current = state;
  // Stale answers (a poll that finishes after a newer one) never replace newer data.
  const seq = useRef(0);
  // An answer that lands after the page is gone changes nothing.
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
      seq.current += 1;
    };
  }, []);

  const load = useCallback(
    async (live: boolean) => {
      if (!data) return;
      const mine = ++seq.current;
      try {
        const o = await data.call("ci.overview", { live });
        if (mine === seq.current) {
          setOverview(o);
          setError(null);
        }
      } catch (e) {
        if (!alive.current) return;
        if (isUnsupported(e)) setUnsupported(true);
        else if (mine === seq.current) setError(message(e));
      }
    },
    [data],
  );

  // First the local records at once, then the same with the service's repositories.
  useEffect(() => {
    void load(false).then(() => load(true));
  }, [load]);

  const busy = overview?.jobs.some((j) => j.state === "queued" || j.state === "running") ?? false;
  useEffect(() => {
    if (!data || unsupported) return;
    const id = setInterval(() => void load(true), busy ? ciTiming.busyPollMs : ciTiming.pollMs);
    return () => clearInterval(id);
  }, [data, unsupported, busy, load]);

  const reduce = useCallback(
    (action: CiAction): CiState | null => {
      const prev = current.current;
      if (!prev) return null;
      const next = ciReduce(core, prev, action) ?? prev;
      current.current = next;
      setState(next);
      return next;
    },
    [core],
  );

  const open = useCallback((url: string) => void session.openExternal(url).catch(() => {}), [session]);

  /** Asks the hosted connect step and reduces its answer. */
  const connectStep = useCallback(
    async (first: boolean) => {
      if (!data) return;
      let answer: CiConnectAnswer;
      try {
        answer = await data.call("ci.connect", {});
      } catch (e) {
        answer = { state: "failed", message: message(e) };
      }
      if (!alive.current) return;
      const before = current.current?.connect.url ?? null;
      reduce({ type: "connectAnswer", answer });
      if (answer.state === "waiting" && answer.url && (first || answer.url !== before)) open(answer.url);
      if (answer.state === "sign_in") void session.signIn().catch(() => {});
      if (answer.state === "connected") void load(true);
    },
    [data, reduce, open, session, load],
  );

  // While the person is on GitHub, the page carries on by itself.
  const waiting = state?.connect.phase === "waiting";
  useEffect(() => {
    if (!waiting) return;
    const id = setInterval(() => void connectStep(false), ciTiming.connectPollMs);
    return () => clearInterval(id);
  }, [waiting, connectStep]);

  const act = useCallback(
    (action: CiAction, extra?: { token?: string }) => {
      const before = current.current;
      const next = reduce(action);
      if (!before || !next) return;
      switch (action.type) {
        case "connectStart":
          void connectStep(true);
          break;
        case "connectClose":
          if (before.connect.phase === "waiting" && data) void data.call("ci.connectCancel", {}).catch(() => {});
          break;
        case "tokenSubmit": {
          if (!data) break;
          const org = before.connect.org.trim();
          data.call("ci.connectToken", { org, ...(extra?.token?.trim() ? { token: extra.token.trim() } : {}) }).then(
            (a) => {
              if (!alive.current) return;
              const ok = a.state === "connected";
              reduce({ type: "tokenDone", ok, message: ok ? null : (a.message ?? null) });
              if (ok) void load(true);
            },
            (e: unknown) => {
              if (alive.current) reduce({ type: "tokenDone", ok: false, message: message(e) });
            },
          );
          break;
        }
        default:
          break;
      }
    },
    [reduce, connectStep, data, load],
  );

  const setRepository = useCallback(
    (name: string, allowed: boolean) => {
      if (!data) return;
      // The switch moves at once; the answer settles it.
      setOverview((o) =>
        o?.repositories.all ? { ...o, repositories: { ...o.repositories, all: o.repositories.all.map((r) => (r.name === name ? { ...r, allowed } : r)) } } : o,
      );
      data.call("ci.repositories", allowed ? { allow: [name] } : { remove: [name] }).then(
        () => void load(true),
        (e: unknown) => {
          if (!alive.current) return;
          setNotice(message(e));
          void load(true);
        },
      );
    },
    [data, load],
  );

  const setGoal = useCallback(
    (target: string, goal: CiGoal) => {
      if (!data) return;
      setOverview((o) => (o ? { ...o, targets: o.targets.map((t) => (t.target === target ? { ...t, goal } : t)) } : o));
      data.call("ci.goal", { target, goal }).then(
        (r) => {
          if (!alive.current) return;
          setNotice(r.applied ? null : "Saved. It applies when the running jobs end and the service restarts.");
          void load(true);
        },
        (e: unknown) => {
          if (!alive.current) return;
          setNotice(message(e));
          void load(true);
        },
      );
    },
    [data, load],
  );

  const showLog = useCallback(
    (ref: { jobKey: string; pool: string; slot: string }) => {
      if (!data) return;
      setLogs((l) => ({ ...l, [ref.jobKey]: { lines: l[ref.jobKey]?.lines ?? null, error: null } }));
      data.call("ci.log", ref).then(
        (r) => {
          if (alive.current) setLogs((l) => ({ ...l, [ref.jobKey]: { lines: r.lines, error: null } }));
        },
        (e: unknown) => {
          if (alive.current) setLogs((l) => ({ ...l, [ref.jobKey]: { lines: null, error: message(e) } }));
        },
      );
    },
    [data],
  );

  const view = overview && state ? ciView(core, overview, state) : null;
  return { view, unsupported, noCore: core.status === "unavailable", error, logs, act, setRepository, setGoal, showLog, open, notice };
}
