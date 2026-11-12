// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * What the first run keeps between launches (`components/first-run-new`
 * reads and writes its progress here, so Reset & Show forgets it), and the
 * Cua CLI's install, which Settings shows too (`firstRun.cli` in
 * `cua-spaces-app-core/src/first_run.rs`).
 */

import { useContext, useEffect, useState, useSyncExternalStore } from "react";

import { BridgeContext } from "./BridgeProvider";
import type { CoreClient } from "./core";
import type { CliInstallPlan, FirstRunCli, FirstRunCliView, FirstRunInput, FirstRunStep } from "./contracts/first-run";
import { extraName, spokenName } from "./space-names";
import type { NewRunSaved, RestOffer } from "../lib/first-run-new";

export { extraName, spokenName };

/** Install the Cua CLI, as the first run and Settings show it (`firstRun.cli`). */
export function firstRunCliView(core: CoreClient, input: Pick<FirstRunInput, "cli" | "cliName">): FirstRunCliView | null {
  return core.tryCall<FirstRunCliView>("firstRun.cli", { input: { ...input } }) ?? null;
}

/** The CLI is on this computer (`FirstRunCli::present`). */
export function cliPresent(cli: FirstRunCli | undefined): boolean {
  return Boolean(cli?.installed || cli?.plan?.upToDate || cli?.plan?.found);
}

/** The app carries a CLI and none is on this computer (`FirstRunCli::missing`). */
export function cliMissing(cli: FirstRunCli | undefined): boolean {
  return !cliPresent(cli) && Boolean(cli?.plan?.source);
}

/* ---- What the page keeps between launches ----------------------------------------- */

/** The first run's progress (resumes after a relaunch, and feeds Home's
 * to-do list after Finish later). */
export interface FirstRunSaved {
  step: FirstRunStep;
  /** An older saved run's one connected agent (setup id). */
  agent: string | null;
  /** The agents connected on Pick your agent. */
  picked: string[] | null;
  /** When the try screen first showed (the access log counts from here). */
  setupStartedMs: number | null;
  /** When the agent was first seen using Cua. */
  usedAtMs: number | null;
  /** The first Space's create (its pending row id). */
  extraPendingId: string | null;
  extraOs: string | null;
  /** "Not now" on the first Space card. */
  extraNotNow: boolean;
  /** Skip for now: the sidebar offers the first run again. */
  finishedLater: boolean;
  /** The to-do list was closed. */
  todosClosed: boolean;
  /** Advanced setup on Pick your agent (CUA-2267). */
  advanced?: boolean;
  /** The first run's progress (`lib/first-run-new.ts`). Kept here so Reset & Show forgets it too. */
  newRun?: Partial<NewRunSaved>;
  /**
   * Home's "Now bring the rest in" (CUA-3150, idea C): `waiting` from the end of setup until the first job is done,
   * `shown` from then until "Not now" (`dismissed`, never again). Absent: setup didn't finish here, so it isn't offered.
   */
  restOffer?: RestOffer | null;
  /** When setup ended: agent runs started from then on count as the first job. */
  restSinceMs?: number | null;
}

const KEY = "cua-spaces:first-run";
const EMPTY: FirstRunSaved = {
  step: "welcome",
  agent: null,
  picked: null,
  setupStartedMs: null,
  usedAtMs: null,
  extraPendingId: null,
  extraOs: null,
  extraNotNow: false,
  finishedLater: false,
  todosClosed: false,
};
/** "Not now" on the plan card: never again (no plan shows in the first run now). */
export const UPSELL_DISMISSED_KEY = "cua.upsell.dismissed.onboarding";
/** The user entered teammates' emails (a share with other people). */
export const TEAMMATES_KEY = "cua.firstRun.teammates";

let saved: FirstRunSaved | undefined;
const listeners = new Set<() => void>();

function storage(): Storage | undefined {
  try {
    return globalThis.localStorage;
  } catch {
    return undefined;
  }
}

export function readFirstRun(): FirstRunSaved {
  if (saved) return saved;
  try {
    const raw = storage()?.getItem(KEY);
    saved = raw ? { ...EMPTY, ...(JSON.parse(raw) as Partial<FirstRunSaved>) } : EMPTY;
  } catch {
    saved = EMPTY;
  }
  return saved;
}

export function writeFirstRun(next: Partial<FirstRunSaved> | null): void {
  saved = next ? { ...readFirstRun(), ...next } : EMPTY;
  try {
    if (next) storage()?.setItem(KEY, JSON.stringify(saved));
    else storage()?.removeItem(KEY);
  } catch {
    // Not kept: the flow still works, it just won't resume.
  }
  for (const l of [...listeners]) l();
}

function setFlag(key: string): void {
  try {
    storage()?.setItem(key, "1");
  } catch {
    // Not kept.
  }
  for (const l of [...listeners]) l();
}

/** The share sheet calls this when the user shares with other people. */
export function noteTeammates(): void {
  setFlag(TEAMMATES_KEY);
}

/** Tests: forget the cached copy. */
export function resetFirstRunCache(): void {
  saved = undefined;
}

const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => void listeners.delete(l);
};

export function useFirstRunSaved(): FirstRunSaved {
  return useSyncExternalStore(subscribe, readFirstRun, readFirstRun);
}

/** The app's bundled CLI and its install (the Command line screen, Settings' row). */
export interface CliInstallHook {
  state: FirstRunCli;
  /** Installs it (adding its folder to PATH only when it is not there), then reads the plan and the settings again. */
  install(): Promise<void>;
}

/** Reads the install plan while `active` (a host without one leaves `plan` null). */
export function useCliInstall(active = true): CliInstallHook {
  const ctx = useContext(BridgeContext);
  if (!ctx) throw new Error("bridge hooks need a <BridgeProvider> above them");
  const { adapter, store } = ctx;
  const [plan, setPlan] = useState<CliInstallPlan | null>(null);
  const [installing, setInstalling] = useState(false);
  const [installed, setInstalled] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    if (!active || !adapter) return;
    let live = true;
    adapter
      .call("cli.plan", {})
      .then((p) => live && setPlan(p))
      .catch(() => live && setPlan(null));
    return () => {
      live = false;
    };
  }, [active, adapter]);
  return {
    state: { plan, installing, installed, error },
    async install() {
      if (!adapter || installing || !plan?.source) return;
      setInstalling(true);
      setError(null);
      let done: CliInstallPlan;
      try {
        done = await adapter.call("cli.install", { modifyPath: !plan.onPath });
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
        setInstalling(false);
        return;
      }
      setInstalled(done.target);
      setPlan(done);
      setInstalling(false);
      // The pages' terminal hints follow the host's `settings.cli`.
      await store?.refresh("settings").catch(() => {});
    },
  };
}
