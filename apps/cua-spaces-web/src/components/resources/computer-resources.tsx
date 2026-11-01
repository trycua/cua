// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Link } from "@tanstack/react-router";

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
  level: "low" | "critical";
  /** This computer: its Spaces can be sorted by what they use. */
  local: boolean;
}

/**
 * What needs action on your computers, nothing else: today a disk nearly or
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
      text: disk.level === "critical" ? `Disk full on ${name} (${size(host.diskFree)} left)` : `Disk almost full on ${name} (${size(host.diskFree)} left)`,
    });
  }
  return out;
}

/** Home's warning lines: one quiet row each, only while there is one, on a soft fill in the one radius (no outline). */
export function ComputerWarnings({ snapshot, machines }: { snapshot: ResourcesSnapshot | null; machines: readonly Machine[] }) {
  const warnings = computerWarnings(snapshot, machines);
  if (!warnings.length) return null;
  return (
    <section data-computer-warnings="" aria-label="Needs attention" className="mb-5 flex flex-col gap-1.5">
      {warnings.map((w) => (
        <p
          key={`${w.kind}:${w.machineId}`}
          role="status"
          data-computer-warning={w.kind}
          data-low-disk={w.level}
          className="flex min-h-9 items-center gap-2.5 rounded-lg bg-tile px-3 py-2 text-[13px] text-muted-foreground"
        >
          <span aria-hidden className={w.level === "critical" ? "size-1.5 shrink-0 rounded-full bg-destructive" : "size-1.5 shrink-0 rounded-full bg-[#d18a12]"} />
          <span className="min-w-0 flex-1">{w.text}</span>
          {w.local ? (
            // The list of this computer's Spaces, biggest first: the cleanup path.
            <Link
              to="/spaces"
              search={{ view: "list", on: w.machineId, sort: "size" }}
              data-see-space-use=""
              className="shrink-0 text-[13px] font-medium text-foreground underline-offset-2 hover:underline"
            >
              See what’s using space
            </Link>
          ) : null}
        </p>
      ))}
    </section>
  );
}
