// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { ComputerResources, SpaceResources } from "@/bridge";
import { cores, diskParts, fill, isRunning, LABELS, lowDiskWarning, onHostLine, size, sizeOf } from "@/lib/resources";
import { cn } from "@/lib/utils";

import { Bar, LowDiskWarning } from "./meters";

/**
 * A Space's full numbers on its page: Processor, Memory and Space for
 * files, each what it was given and what it uses now, then "How it fits on
 * this Mac".
 */
export function SpaceResourcesPanel({
  space,
  name,
  host,
  computer,
  siblings,
}: {
  space: SpaceResources;
  /** The Space's name ("macOS 1"). */
  name: string;
  /** Its computer's numbers, when it sent them. */
  host: ComputerResources | null;
  /** "this Mac" (`thisComputerText`), or the other computer's name. */
  computer: string;
  /** Every Space on that computer, with its name, for the disk bar. */
  siblings: { id: string; name: string; bytes: number }[];
}) {
  const u = isRunning(space) ? space.usage : null;
  const whose = `${computer}’s`;
  const memTotal = space.memoryAllocated || u?.memoryTotal || 0;
  const diskTotal = space.diskAllocated || u?.diskTotal || 0;
  const cards = [
    {
      label: LABELS.cpu,
      big: u ? `${Math.round(u.cpuPercent)}% busy` : "Stopped",
      given: space.cpuAllocated ? (host ? `Given ${space.cpuAllocated} of ${whose} ${cores(host.cpuCount)}` : `Given ${cores(space.cpuAllocated)}`) : null,
      fill: u ? fill(u.cpuPercent, 100) : 0,
      note: u ? "Busy means how hard it’s working right now." : "It uses no processor while stopped.",
    },
    {
      label: LABELS.memory,
      big: u ? (memTotal ? sizeOf(u.memoryUsed, memTotal) : size(u.memoryUsed)) : "Off",
      given: space.memoryAllocated ? (host ? `Given ${size(space.memoryAllocated)} of ${whose} ${size(host.memoryTotal)}` : `Given ${size(space.memoryAllocated)}`) : null,
      fill: u ? fill(u.memoryUsed, memTotal) : 0,
      note: u ? "The rest is free for its apps." : "Its memory goes back while it’s stopped.",
    },
    {
      label: LABELS.disk,
      big: u && diskTotal ? sizeOf(u.diskUsed, diskTotal) : diskTotal ? `Up to ${size(diskTotal)}` : "—",
      given: u ? `What ${name} sees inside` : null,
      fill: u ? fill(u.diskUsed, diskTotal) : 0,
      note: onHostLine(space, computer),
    },
  ];
  return (
    <section data-space-resources="" aria-label="Resources" className="mt-6">
      <div className="grid gap-4 md:grid-cols-3">
        {cards.map((c) => (
          <div key={c.label} data-resource-card={c.label} className="rounded-xl border bg-card p-4 shadow-xs">
            <h3 className="text-xs text-muted-foreground">{c.label}</h3>
            <p className="mt-1.5 text-xl leading-tight font-semibold tracking-[-0.01em] tabular-nums">{c.big}</p>
            {c.given ? <p className="mt-1 text-xs text-muted-foreground">{c.given}</p> : null}
            <Bar fill={c.fill} className="mt-3.5" />
            {c.note ? <p className="mt-3 text-xs">{c.note}</p> : null}
          </div>
        ))}
      </div>
      {host ? <DiskFit host={host} computer={computer} spaceId={space.id} siblings={siblings} /> : null}
      <p className="mt-4 px-1 text-xs text-muted-foreground">
        Updates every 10 seconds while this window is open, and every 5 minutes in the background. Cua reads numbers only. It never looks at your file names.
      </p>
    </section>
  );
}

/** "How it fits on this Mac": your own files, each Space, and free space, on one bar. */
function DiskFit({ host, computer, spaceId, siblings }: { host: ComputerResources; computer: string; spaceId: string; siblings: { id: string; name: string; bytes: number }[] }) {
  const parts = diskParts(host, siblings);
  const total = host.diskTotal || 1;
  const warning = lowDiskWarning(host, computer.charAt(0).toUpperCase() + computer.slice(1));
  const segments = [
    { key: "own", label: "Your own files and apps", bytes: parts.own, className: "bg-muted-foreground/60" },
    ...siblings.map((s) => ({ key: s.id, label: s.name, bytes: s.bytes, className: s.id === spaceId ? "bg-brand-strong" : "bg-brand" })),
  ];
  return (
    <div data-disk-fit="" className="mt-4 rounded-xl border bg-card p-4 shadow-xs">
      <div className="flex items-baseline justify-between gap-3">
        <h3 className="text-[13px] font-semibold">How it fits on {computer}</h3>
        <span className="text-xs text-muted-foreground">{size(host.diskTotal)}</span>
      </div>
      <div className="mt-3 flex h-2 overflow-hidden rounded-full bg-foreground/10" role="img" aria-label={`${size(host.diskFree)} free of ${size(host.diskTotal)}`}>
        {segments.map((s) => (
          <div key={s.key} className={cn("h-full", s.className)} style={{ width: `${(s.bytes / total) * 100}%` }} />
        ))}
      </div>
      <ul className="mt-3 flex flex-wrap gap-x-5 gap-y-1.5 text-xs">
        {segments.map((s) => (
          <li key={s.key} data-disk-part={s.key} className="flex items-center gap-1.5">
            <span className={cn("size-2 rounded-xs", s.className)} aria-hidden />
            {s.label} · {size(s.bytes)}
          </li>
        ))}
        <li data-disk-part="free" className="flex items-center gap-1.5">
          <span className="size-2 rounded-xs border bg-foreground/10" aria-hidden />
          Free · {size(parts.free)}
        </li>
      </ul>
      {warning ? <LowDiskWarning warning={warning} compact className="mt-4 rounded-lg border" /> : null}
    </div>
  );
}
