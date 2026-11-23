// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect, useState } from "react";

import { useAgents, type AgentSetupRow } from "@/bridge";

import { TASK_AGENTS } from "./task-agents";

/**
 * The task agents (harness ids) on this computer, from the agent setup's
 * detection: `installed` there counts the app itself or files it wrote, never
 * a file Cua wrote for it (CUA-3063). Being set up for Cua (`configured`) is
 * not enough. Null until the setup answered; empty when it could not.
 */
export function installedTaskAgents(rows: readonly AgentSetupRow[]): ReadonlySet<string> {
  const found = new Set(rows.filter((r) => r.installed).map((r) => r.agent));
  return new Set(TASK_AGENTS.filter((a) => found.has(a.setupId)).map((a) => a.id));
}

export function useInstalledAgents(): ReadonlySet<string> | null {
  const { agentSetup } = useAgents();
  const [installed, setInstalled] = useState<ReadonlySet<string> | null>(null);
  useEffect(() => {
    let live = true;
    agentSetup()
      .then((rows) => {
        if (live) setInstalled(installedTaskAgents(rows));
      })
      .catch(() => {
        if (live) setInstalled(new Set());
      });
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  return installed;
}
