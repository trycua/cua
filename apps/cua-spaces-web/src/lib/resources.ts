// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The words and bar fills for processor, memory and disk numbers
 * (CUA-1880). Plain labels only: "Processor", "Memory", "Space for files",
 * "busy", "in use", "free". Never vCPU, RSS, APFS, sparse or cgroup.
 */

import type { ComputerResources, SpaceResources } from "@/bridge";

export const LABELS = { cpu: "Processor", memory: "Memory", disk: "Space for files" } as const;

const GB = 1_000_000_000;

/** A number of GB as people read it: "3.1", "41", "8". */
function gbNumber(bytes: number): string {
  const tenths = Math.round((bytes / GB) * 10) / 10;
  return tenths >= 9.95 || Number.isInteger(tenths) ? String(Math.round(tenths)) : tenths.toFixed(1);
}

/** "41 GB", "3.1 GB", "1 TB", "1.5 TB". */
export function size(bytes: number): string {
  if (bytes >= 999.5 * GB) {
    const tb = Math.round(bytes / (GB * 100)) / 10;
    return `${Number.isInteger(tb) ? tb.toFixed(0) : tb.toFixed(1)} TB`;
  }
  return `${gbNumber(bytes)} GB`;
}

/** "3.1 of 8 GB" (same unit) or "800 GB of 1 TB". */
export function sizeOf(used: number, total: number): string {
  const whole = size(total);
  const unit = whole.split(" ")[1];
  const part = size(used);
  return part.endsWith(` ${unit}`) ? `${part.split(" ")[0]} of ${whole}` : `${part} of ${whole}`;
}

export const cores = (n: number) => `${n} ${n === 1 ? "core" : "cores"}`;

/** A bar's fill, 0 to 1. */
export const fill = (part: number | null | undefined, whole: number | null | undefined) =>
  part == null || !whole ? 0 : Math.min(1, Math.max(0, part / whole));

/** How long ago another computer measured: "just now", "2 min ago", "3 hours ago". */
export function updatedAgo(sampledAtMs: number, now: number): string {
  const min = Math.floor((now - sampledAtMs) / 60_000);
  if (min < 1) return "just now";
  if (min < 60) return `${min} min ago`;
  const h = Math.floor(min / 60);
  if (h < 24) return `${h} ${h === 1 ? "hour" : "hours"} ago`;
  const d = Math.floor(h / 24);
  return `${d} ${d === 1 ? "day" : "days"} ago`;
}

export interface Meter {
  label: string;
  value: string;
  /** 0 to 1. */
  fill: number;
  tone?: "warning" | "critical";
  /** The longer words, for a tooltip. */
  help?: string;
}

const tone = (c: ComputerResources): Meter["tone"] => (c.warning.level === "critical" ? "critical" : c.warning.level === "low" ? "warning" : undefined);

/** A computer's three meters: "12 cores · 38% busy", "41 of 64 GB in use", "41 GB free of 1 TB". */
export function hostMeters(c: ComputerResources): Meter[] {
  return [
    { label: LABELS.cpu, value: `${cores(c.cpuCount)} · ${Math.round(c.cpuPercent)}% busy`, fill: fill(c.cpuPercent, 100) },
    { label: LABELS.memory, value: `${sizeOf(c.memoryUsed, c.memoryTotal)} in use`, fill: fill(c.memoryUsed, c.memoryTotal) },
    { label: LABELS.disk, value: `${size(c.diskFree)} free of ${size(c.diskTotal)}`, fill: fill(c.diskTotal - c.diskFree, c.diskTotal), tone: tone(c) },
  ];
}

/** "Your extra Spaces use 6 cores, 12 GB of memory and about 25 GB for files." Null with none. */
export function extraSpacesLine(c: ComputerResources, where: "here" | "there"): string | null {
  if (!c.spacesCount) return null;
  return `Your extra Spaces${where === "there" ? " there" : ""} use ${cores(c.spacesCpuAllocated)}, ${size(c.spacesMemoryAllocated)} of memory and about ${size(c.spacesDiskBytes)} for files.`;
}

export const isRunning = (s: SpaceResources) => s.powerState === "running";

/**
 * What a Space costs this Mac's disk, by the contract's rule: "Uses about
 * 19 GB on this Mac", plus "(shares space with other copies)" when it does;
 * "Shares space with other copies" when it couldn't be measured; null when
 * there's nothing to say.
 */
export function onHostLine(s: SpaceResources, computer = "this Mac"): string | null {
  const d = s.diskOnHost;
  if (!d) return null;
  if (d.bytes == null) return d.sharesSpace ? "Shares space with other copies." : null;
  return `Uses about ${size(d.bytes)} on ${computer}${d.sharesSpace ? " (shares space with other copies)" : ""}.`;
}

/** A Space's three short meters on its tile: "22% busy", "3.1 of 8 GB", "About 19 GB" (the
 * tooltip and the Space's page say the rest). */
export function spaceMeters(s: SpaceResources): Meter[] {
  const on = isRunning(s) && s.usage !== null;
  const u = s.usage;
  const memTotal = s.memoryAllocated || u?.memoryTotal || 0;
  const onHost = s.diskOnHost?.bytes ?? null;
  return [
    {
      label: LABELS.cpu,
      value: on ? `${Math.round(u!.cpuPercent)}% busy` : "Stopped",
      fill: on ? fill(u!.cpuPercent, 100) : 0,
      help: s.cpuAllocated ? (on ? `${cores(s.cpuAllocated)} · ${Math.round(u!.cpuPercent)}% busy` : `Given ${cores(s.cpuAllocated)}`) : undefined,
    },
    {
      label: LABELS.memory,
      value: on && memTotal ? sizeOf(u!.memoryUsed, memTotal) : on ? size(u!.memoryUsed) : "Off",
      fill: on ? fill(u!.memoryUsed, memTotal) : 0,
      help: s.memoryAllocated ? `Given ${size(s.memoryAllocated)}` : undefined,
    },
    {
      label: LABELS.disk,
      value: onHost != null ? `About ${size(onHost)}` : s.diskAllocated ? `Up to ${size(s.diskAllocated)}` : "—",
      fill: fill(onHost, s.diskAllocated),
      help: onHostLine(s) ?? undefined,
    },
  ];
}

/** The low-disk warning's words, or null when there's room. */
export function lowDiskWarning(c: ComputerResources, computer: string): { level: "low" | "critical"; title: string; detail: string } | null {
  const level = c.warning.level;
  if (level === "ok") return null;
  const free = `${size(c.diskFree)} free of ${size(c.diskTotal)}.`;
  return level === "critical"
    ? { level, title: `${computer} is out of space. Delete old Spaces or snapshots.`, detail: `${free} New Spaces can’t start until there’s more room.` }
    : { level, title: `${computer} is almost out of space. Delete old Spaces or snapshots.`, detail: `${free} New Spaces may not fit, and apps can slow down.` };
}

/** The pieces of "How it fits on this Mac": your own files, each Space, and free. */
export function diskParts(c: ComputerResources, spaces: { name: string; bytes: number }[]): { own: number; free: number } {
  const used = c.diskTotal - c.diskFree;
  const inSpaces = spaces.reduce((n, s) => n + s.bytes, 0);
  return { own: Math.max(0, used - inSpaces), free: c.diskFree };
}
