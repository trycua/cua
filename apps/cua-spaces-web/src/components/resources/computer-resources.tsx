// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { hostResources, type Machine, type ResourcesSnapshot } from "@/bridge";
import { thisComputerLabel, thisComputerText } from "@/lib/host-labels";
import { extraSpacesLine, hostMeters, lowDiskWarning, updatedAgo } from "@/lib/resources";

import { LowDiskWarning, MeterRow } from "./meters";

/**
 * Home's strip for each computer that sent numbers: This Mac first (with
 * the low-disk warning on top when it's short of space), then your other
 * computers' summaries with how old they are. Nothing when no computer
 * sent any.
 */
export function ComputersResources({ snapshot, machines, now = Date.now() }: { snapshot: ResourcesSnapshot | null; machines: readonly Machine[]; now?: number }) {
  const shown = machines.map((m) => ({ machine: m, host: hostResources(snapshot, m) })).filter((x) => x.host);
  if (!shown.length) return null;
  return (
    <section data-computers-resources="" aria-label="Computers" className="mb-6 space-y-3">
      {shown.map(({ machine, host }) => {
        const here = machine.current;
        const name = here ? thisComputerLabel() : machine.name;
        const warning = lowDiskWarning(host!, name);
        const extra = extraSpacesLine(host!, here ? "here" : "there");
        const line = here ? extra : [`Updated ${updatedAgo(host!.sampledAtMs, now)}.`, extra].filter(Boolean).join(" ");
        return (
          <div key={machine.id} data-computer-resources={machine.id} className="overflow-hidden rounded-xl border bg-card shadow-xs">
            <div className="flex items-baseline justify-between gap-3 px-4 pt-3">
              <h2 className="truncate text-[13px] font-semibold">{here && machine.name !== name ? `${machine.name} (${thisComputerText()})` : machine.name}</h2>
              <span className="shrink-0 text-xs text-muted-foreground">{machine.online ? "Online" : "Offline"}</span>
            </div>
            {warning ? <LowDiskWarning warning={warning} className="mt-3 border-y" /> : null}
            <div className="grid gap-x-8 gap-y-3 px-4 pt-3 sm:grid-cols-3">
              {hostMeters(host!).map((m) => (
                <MeterRow key={m.label} meter={m} />
              ))}
            </div>
            {line ? <p className="px-4 pt-2.5 pb-3 text-xs text-muted-foreground">{line}</p> : <div className="pb-3" />}
          </div>
        );
      })}
    </section>
  );
}
