// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect, useState } from "react";

import {
  driverCopy,
  isUnsupported,
  setupSummary,
  useBridge,
  type AgentSetupOutcome,
  type AgentSetupRow,
  type AgentSetupSummary,
  type DriverCopy,
} from "@/bridge";

export interface AgentsStep {
  copy: DriverCopy;
  /** Installed coding agents; null while looking. */
  agents: AgentSetupRow[] | null;
  selected: Set<string>;
  toggle(id: string, on: boolean): void;
  /** The cua skills and MCP server (`agents.configure`). */
  skills: boolean;
  setSkills(on: boolean): void;
  /** The cua-driver card. */
  driver: boolean;
  setDriver(on: boolean): void;
  busy: boolean;
  error: string | null;
  canSetUp: boolean;
  setUp(): Promise<void>;
  /** One line per ticked agent once set up. */
  summaries: AgentSetupSummary[] | null;
  /** The agents that set up cleanly (Done's "AI agents"). */
  configured: string[];
}

/**
 * The first run's AI agents page, as `OnboardingModel` runs it: detect the
 * coding agents, then for the ticked ones add the cua skills and MCP server
 * and, with the card ticked, cua-driver. The lines after are the core's
 * (`agents.setupSummary`).
 */
export function useAgentsStep(active: boolean): AgentsStep {
  const { core, data } = useBridge();
  const copy = driverCopy(core);
  const [agents, setAgents] = useState<AgentSetupRow[] | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [skills, setSkills] = useState(true);
  const [driver, setDriver] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [summaries, setSummaries] = useState<AgentSetupSummary[] | null>(null);
  const [names, setNames] = useState<string[]>([]);

  useEffect(() => {
    if (!active || !data || agents) return;
    let live = true;
    data.call("agents.setup", {}).then(
      (rows) => {
        if (!live) return;
        const installed = rows.filter((r) => r.installed);
        setAgents(installed);
        setSelected(new Set(installed.map((r) => r.agent)));
      },
      () => live && setAgents([]),
    );
    return () => {
      live = false;
    };
  }, [active, data, agents]);

  const ticked = (agents ?? []).filter((a) => selected.has(a.agent));
  const setUp = async () => {
    if (!data || ticked.length === 0) return;
    setBusy(true);
    setError(null);
    try {
      const ids = ticked.map((a) => a.agent);
      const rows = skills ? await data.call("agents.configure", { agents: ids }) : [];
      let outcomes: AgentSetupOutcome[] = [];
      let driverMissing = false;
      if (driver) {
        outcomes = await data.call("agents.setupDriver", { agents: ids }).catch((e: unknown) => {
          if (!isUnsupported(e)) throw e;
          driverMissing = true;
          return [];
        });
      }
      setNames(ticked.map((a) => a.name));
      setSummaries(
        ticked.map((a) => {
          const s = setupSummary(core, outcomes, a.agent, a.name);
          const detail = rows.find((r) => r.agent === a.agent)?.detail;
          return { ...s, text: [detail, s.text].filter((t) => t && t !== "nothing to change").join(" · ") || s.text };
        }),
      );
      if (driverMissing) setError("cua-driver needs the Cua Spaces app on this machine. Set it up later in Settings.");
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  return {
    copy,
    agents,
    selected,
    toggle: (id, on) =>
      setSelected((s) => {
        const next = new Set(s);
        if (on) next.add(id);
        else next.delete(id);
        return next;
      }),
    skills,
    setSkills,
    driver,
    setDriver,
    busy,
    error,
    canSetUp: ticked.length > 0 && (skills || driver) && !busy,
    setUp,
    summaries,
    configured: names.filter((_, i) => summaries?.[i]?.failed.length === 0),
  };
}
