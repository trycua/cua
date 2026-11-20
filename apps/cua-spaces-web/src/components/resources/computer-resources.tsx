// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { hostResources, type Machine, type ResourcesSnapshot } from "@/bridge";
import { thisComputerLabel } from "@/lib/host-labels";
import { lowDiskWarning, size } from "@/lib/resources";

/** One thing on a computer that needs action, as Home says it. */
export interface ComputerWarning {
  /** `disk`; the same line takes permissions, updates and the budget once
   * the app reports them per computer. */
  kind: "disk";
  machineId: string;
  text: string;
  /** "33 GB left", for the computer's own row. */
  free: string;
  level: "low" | "critical";
  /** This computer: its Spaces can be sorted by what they use. */
  local: boolean;
}

/**
 * What needs action on your computers, nothing else (the Computers page marks it on the computer's own row): today a disk nearly or
 * completely full ("Disk almost full on This Mac (12 GB left)"). A computer
 * that is just offline is not a warning, and nothing shows while all is well.
 *
 * TODO: permissions needed, an update required and a budget
 * reached go through this same line once the app reports them per computer.
 */
export function computerWarnings(snapshot: ResourcesSnapshot | null, machines: readonly Machine[]): ComputerWarning[] {
  const out: ComputerWarning[] = [];
  for (const m of machines) {
    const host = hostResources(snapshot, m);
    if (!host) continue;
    const name = m.current ? thisComputerLabel() : m.name;
    const disk = lowDiskWarning(host, name);
    if (!disk) continue;
    out.push({
      kind: "disk",
      machineId: m.id,
      level: disk.level,
      local: Boolean(m.current),
      free: size(host.diskFree),
      text: disk.level === "critical" ? `Disk full on ${name} (${size(host.diskFree)} left)` : `Disk almost full on ${name} (${size(host.diskFree)} left)`,
    });
  }
  return out;
}
