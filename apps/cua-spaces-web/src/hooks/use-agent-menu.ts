// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Which coding apps Home's agent menu shows. The rule (the defaults, the last
 * app that cannot be hidden, a connect that shows an app again) is the app
 * core's `agent_menu`; the choice is kept by the host with the rest of the
 * app's settings (`settings.get` answers `values.agentMenu`, `settings.set`
 * with key `agentMenu` saves it), so the web, SwiftUI and Electron apps
 * agree and it survives a restart. A host that keeps no core settings
 * refuses the save: the page then keeps the choice for this session.
 */

import { useCallback, useEffect, useMemo, useState } from "react";

import {
  agentMenuChoose,
  agentMenuSync,
  agentMenuView,
  useBridge,
  useSettings,
  type AgentMenuChange,
  type AgentMenuPrefs,
  type AgentMenuSettings,
} from "@/bridge";
import type { Connectors } from "@/hooks/use-connectors";

const NONE: AgentMenuPrefs = { shown: [], hidden: [], seen: [] };

const same = (a: AgentMenuPrefs, b: AgentMenuPrefs) => JSON.stringify(a) === JSON.stringify(b);

export interface AgentMenu {
  /** The choice now, with a connect since the last look already counted (null before the sign-ins are read: use `prefs`). */
  prefs: AgentMenuPrefs;
  /** The rows of the switches (the core's). */
  view: AgentMenuSettings | null;
  /** Switches an app on or off; answers the core's words when it was refused (the last app). */
  choose(app: string, on: boolean): Promise<string | null>;
}

/** The agent menu's choice and what changes it. */
export function useAgentMenu(c: Pick<Connectors, "input" | "keysReady">): AgentMenu {
  const { core } = useBridge();
  const settings = useSettings();
  const [session, setSession] = useState<AgentMenuPrefs | null>(null);
  const stored = session ?? settings.data?.values.agentMenu ?? NONE;
  const ready = c.keysReady && Boolean(settings.data);
  // A hidden app that gained an account since the last look is shown again (not on every refresh).
  const prefs = useMemo(() => (ready ? (agentMenuSync(core, c.input, stored) ?? stored) : stored), [ready, core, c.input, stored]);
  const save = useCallback(
    async (next: AgentMenuPrefs) => {
      if (session) return void setSession(next);
      try {
        await settings.updateSetting("agentMenu", next);
      } catch {
        setSession(next);
      }
    },
    [session, settings],
  );
  useEffect(() => {
    if (ready && !same(prefs, stored)) void save(prefs);
  }, [ready, prefs, stored, save]);
  const view = useMemo(() => agentMenuView(core, c.input, prefs), [core, c.input, prefs]);
  const choose = useCallback(
    async (app: string, on: boolean): Promise<string | null> => {
      const r: AgentMenuChange | null = agentMenuChoose(core, c.input, prefs, app, on);
      if (!r) return null;
      if (r.refused) return r.refused;
      await save(r.prefs);
      return null;
    },
    [core, c.input, prefs, save],
  );
  return { prefs, view, choose };
}
