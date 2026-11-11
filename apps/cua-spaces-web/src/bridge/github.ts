// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Settings → Connections → GitHub: the sign-in Cua uses when an agent asks
 * to push a branch and open a pull request. Every word of a row and of the
 * page behind it is the app core's (`githubConnection.view`,
 * `cua-spaces-app-core/src/github_connection.rs`); this module reads the
 * daemon's status through the host (`ops/github.ts`) and runs the
 * actions: Rescan, choosing a gh account, switching a host off.
 */

import { useCallback, useEffect, useRef, useState } from "react";

import type { CoreClient } from "./core";
import type { GithubView } from "./contracts/github";
import { useBridge } from "./hooks";
import type { GithubReport } from "./ops/github";

/** The section the core draws for the daemon's status (`error`: why there is none). */
export function githubView(core: CoreClient, report: GithubReport | null, error: string | null): GithubView | null {
  return (
    core.tryCall<GithubView>("githubConnection.view", {
      input: { ...(report ?? { hosts: [], ghFound: false, ghOld: false, canSave: false }), ...(error ? { error } : {}) },
    }) ?? null
  );
}

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

export interface GithubHook {
  /** The section as the core draws it; null until the first read (or without the core). */
  view: GithubView | null;
  /** The status could not be read, in a sentence. */
  error: string | null;
  /** A rescan is running. */
  rescanning: boolean;
  /** Looks again (gh's accounts, and what a saved or environment token may do). */
  rescan(): void;
  /** Uses this gh account on a host; `null`: gh's own. */
  choose(host: string, login: string | null): void;
  /** Switches a host on or off. */
  setEnabled(host: string, enabled: boolean): void;
  /** What the last action said when it failed, once. */
  notice: string | null;
}

/** Settings, Connections, GitHub. `saved` changes when the pasted token is saved or removed: the status is read again. */
export function useGithub(active = true, saved = false): GithubHook {
  const { core, data } = useBridge();
  const [report, setReport] = useState<GithubReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [rescanning, setRescanning] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const seq = useRef(0);

  const run = useCallback(async (call: () => Promise<GithubReport>, quiet = false) => {
    const mine = ++seq.current;
    try {
      const r = await call();
      if (mine === seq.current) {
        setReport(r);
        setError(null);
      }
    } catch (e) {
      if (mine !== seq.current) return;
      if (quiet) setError(message(e));
      else setNotice(message(e));
    }
  }, []);

  // Opening the page reads the status at once (nothing is looked up on the network).
  useEffect(() => {
    if (!data || !active) return;
    void run(() => data.call("github.status", {}), true);
  }, [data, active, saved, run]);

  const rescan = useCallback(() => {
    if (!data) return;
    setRescanning(true);
    void run(() => data.call("github.rescan", {})).finally(() => setRescanning(false));
  }, [data, run]);
  const choose = useCallback((host: string, login: string | null) => data && void run(() => data.call("github.use", { host, login })), [data, run]);
  const setEnabled = useCallback((host: string, enabled: boolean) => data && void run(() => data.call("github.enable", { host, enabled })), [data, run]);

  const [view, setView] = useState<GithubView | null>(null);
  // The core loads after the first render in a browser: the section waits for it.
  useEffect(() => {
    if (!report && !error) return;
    setView(githubView(core, report, error));
  }, [core, core.status, report, error]);

  return { view, error, rescanning, rescan, choose, setEnabled, notice };
}
