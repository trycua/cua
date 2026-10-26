// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useLocation } from "@tanstack/react-router";
import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from "react";

import { isUnsupported, useBridge, type AgentSetupRow, type DataAdapter } from "@/bridge";
import { toast, toastError } from "@/components/ui/toast";
import { failureText as failureOf, planSync, readState, withSwitch, writeState, type ConnectedAppsState } from "@/lib/connected-apps";

/* ---- What Cua remembers: one local key, shared by Settings, onboarding and the sync ---- */

const storage = () => (typeof localStorage === "undefined" ? null : localStorage);
let state: ConnectedAppsState = readState(storage());
const listeners = new Set<() => void>();

export const connectedAppsState = () => state;

export function setConnectedAppsState(next: ConnectedAppsState): void {
  state = next;
  writeState(storage(), next);
  for (const l of listeners) l();
}

/** Tests start from a clean slate. */
export function resetConnectedAppsState(): void {
  state = readState(storage());
  for (const l of listeners) l();
}

export function useConnectedAppsState(): ConnectedAppsState {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => state,
    () => state,
  );
}

/* ---- Doing it ---- */

/** The apps on this computer; null when this app can't look (nothing to show then). */
async function readRows(data: DataAdapter): Promise<AgentSetupRow[] | null> {
  try {
    return await data.call("agents.setup", {});
  } catch (e) {
    if (isUnsupported(e)) return null;
    throw e;
  }
}

/**
 * Cua in `agent`'s settings, through the same Connect and Disconnect as
 * onboarding, the CLI and MCP (`agents.connect` / `agents.disconnect`, one
 * honest status): on adds Cua and copies the app's sign-in to Keyvault (with
 * Background computer use, best effort); off takes out only what Cua added,
 * the copied sign-in too.
 */
async function setApp(data: DataAdapter, row: AgentSetupRow, on: boolean): Promise<AgentSetupRow[]> {
  if (on) {
    await data.call("agents.connect", { agents: [row.agent], signIn: true });
    try {
      // One switch, one consent: the same apps may also use the computer in the background.
      await data.call("agents.setupDriver", { agents: [row.agent] });
    } catch {
      // Cua Driver is its own install; without it the app still has Cua.
    }
  } else {
    await data.call("agents.disconnect", { agents: [row.agent], signIn: true });
  }
  return (await readRows(data)) ?? [];
}

export interface ConnectedApps {
  /** Null until read; an empty list when this app can't look. */
  rows: AgentSetupRow[] | null;
  /** The apps with a change running. */
  pending: string[];
  /** Why the last change to an app failed (a sentence), by app. */
  failures: Record<string, string>;
  state: ConnectedAppsState;
  toggle(row: AgentSetupRow, on: boolean): Promise<void>;
  /** "Set up new apps automatically". */
  setAuto(on: boolean): void;
  reload(): Promise<void>;
}

/** Settings, Connectors. */
export function useConnectedApps(): ConnectedApps {
  const { data } = useBridge();
  const app = useConnectedAppsState();
  const [rows, setRows] = useState<AgentSetupRow[] | null>(null);
  const [pending, setPending] = useState<string[]>([]);
  const [failures, setFailures] = useState<Record<string, string>>({});

  const reload = useCallback(async () => {
    if (!data) return;
    try {
      setRows((await readRows(data)) ?? []);
    } catch {
      setRows((r) => r ?? []);
    }
  }, [data]);

  useEffect(() => {
    void reload();
    window.addEventListener("focus", reload);
    return () => window.removeEventListener("focus", reload);
  }, [reload]);

  const toggle = useCallback(
    async (row: AgentSetupRow, on: boolean) => {
      if (!data || pending.includes(row.agent)) return;
      setPending((p) => [...p, row.agent]);
      setFailures(({ [row.agent]: _gone, ...rest }) => rest);
      try {
        const after = await setApp(data, row, on);
        setRows(after);
        const now = after.find((r) => r.agent === row.agent);
        if (on && !now?.configured) {
          setFailures((f) => ({ ...f, [row.agent]: failureOf(row.name, now?.detail ?? "") }));
          return;
        }
        setConnectedAppsState(withSwitch(connectedAppsState(), row.agent, on));
        if (!on) {
          toast(`${row.name} turned off`, {
            description: `Cua took itself out of ${row.name}'s settings. Restart ${row.name} to finish. Its past runs stay on the Agents page.`,
            fromCua: true,
            group: `app:${row.agent}`,
            action: { label: "Undo", onClick: () => void toggle(row, true) },
          });
        }
      } catch (e) {
        toastError(`Couldn't change ${row.name}`)(e);
      } finally {
        setPending((p) => p.filter((a) => a !== row.agent));
      }
    },
    [data, pending],
  );

  return { rows, pending, failures, state: app, toggle, setAuto: (on) => setConnectedAppsState({ ...connectedAppsState(), auto: on }), reload };
}

/**
 * Keeps new coding apps connected: when the app starts and when its window
 * gets focus, read the apps on this computer and set up any newly installed
 * one the person said yes to, with a notice and Undo. Never touches an app
 * that was turned off, and does nothing during the first run (it asks there).
 */
export function useAgentSync(): void {
  const { data } = useBridge();
  const { pathname } = useLocation();
  const busy = useRef(false);
  const inOnboarding = pathname.startsWith("/onboarding");

  useEffect(() => {
    if (!data || inOnboarding) return;
    const run = async () => {
      if (busy.current) return;
      busy.current = true;
      try {
        const rows = await readRows(data);
        if (!rows) return;
        const plan = planSync(rows, connectedAppsState());
        setConnectedAppsState(plan.state);
        if (plan.setUp.length === 0) return;
        // Cua's tools only: copying a sign-in waits for the person's own Connect.
        await data.call("agents.connect", { agents: plan.setUp, signIn: false });
        const after = (await readRows(data)) ?? [];
        for (const id of plan.setUp) {
          const row = after.find((r) => r.agent === id);
          if (!row?.configured) continue;
          toast(`${row.name} can now use Cua`, {
            description: `Cua added itself to ${row.name}'s settings. Restart ${row.name} to use it.`,
            fromCua: true,
            group: `app:${id}`,
            action: {
              label: "Undo",
              onClick: () => {
                setConnectedAppsState(withSwitch(connectedAppsState(), id, false));
                void data.call("agents.disconnect", { agents: [id], signIn: false }).catch(toastError(`Couldn't undo ${row.name}`));
              },
            },
          });
        }
      } catch {
        // A read that fails is tried again at the next focus.
      } finally {
        busy.current = false;
      }
    };
    void run();
    window.addEventListener("focus", run);
    return () => window.removeEventListener("focus", run);
  }, [data, inOnboarding]);
}
