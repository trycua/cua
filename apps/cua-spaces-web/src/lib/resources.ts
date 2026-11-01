// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The words and bar fills for processor, memory and disk numbers
 * (CUA-1880). Plain labels only: "Processor", "Memory", "Space for files",
 * "busy", "in use", "free". Never vCPU, RSS, APFS, sparse or cgroup.
 */

import type { ComputerResources, SharedBase, SpaceResources } from "@/bridge";

export const LABELS = { cpu: "Processor", memory: "Memory", disk: "Space for files" } as const;

const MIB = 1024 * 1024;
/** Sizes at or above this are "unlimited" or "unknown" sentinels, never sizes (2^60). */
export const SIZE_SENTINEL_FLOOR = 2 ** 60;

/** A size, or null when it is 0, negative, not a number or a sentinel. */
export function knownSize(bytes: number | null | undefined): number | null {
  return typeof bytes === "number" && Number.isFinite(bytes) && bytes > 0 && bytes < SIZE_SENTINEL_FLOOR ? bytes : null;
}

/**
 * A size in binary units labelled the way macOS labels memory: "512 MB",
 * "2.1 GB", "4 GB", "1.8 TB"; under 1 MB, "244 KB" (never "0 GB" for
 * something). The app core's `format_size`, the same everywhere.
 */
export function size(bytes: number): string {
  const units = ["MB", "GB", "TB", "PB"];
  let v = bytes / MIB;
  if (v < 1) return `${Math.round(bytes / 1024)} KB`;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i += 1;
  }
  const text = v >= 100 ? String(Math.round(v)) : (Math.round(v * 10) / 10).toFixed(1).replace(/\.0$/, "");
  return `${text} ${units[i]}`;
}

/** "3.1 of 8 GB" (same unit) or "717 MB of 4 GB": the core's `of_text`. */
export function sizeOf(used: number, total: number): string {
  const whole = size(total);
  const unit = whole.split(" ")[1];
  const part = size(Math.min(used, total));
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

/* ---- A Space's resources dialog (the core's `resources.spacePanel`) ---------------- */

export interface SpacePanelCard {
  label: string;
  /** The main number: "13% busy", "717 MB of 4 GB", "244 KB". */
  value: string;
  /** "Given 2 of this Mac’s 18 cores". */
  given: string | null;
  /** The bar, 0 to 1; null: nothing to fill it against. */
  fraction: number | null;
}

export type DiskFitKind = "yours" | "thisSpace" | "otherSpaces" | "base" | "free";

export interface DiskFitPart {
  key: string;
  kind: DiskFitKind;
  label: string;
  value: string;
  /** For a base: "shared by 3 Spaces", "only this Space uses it". */
  detail: string | null;
  bytes: number;
  /** Its share of the bar; the parts add up to 1. */
  fraction: number;
}

export interface DiskFitView {
  total: string;
  parts: DiskFitPart[];
  /** What could not be told apart, in plain words. */
  note: string | null;
  warning: { text: string; button: string; critical: boolean } | null;
}

export interface SpacePanelView {
  cards: SpacePanelCard[];
  fit: DiskFitView | null;
}

export interface SpacePanelInput {
  space: SpaceResources;
  name: string;
  host: ComputerResources | null;
  /** "this Mac", or the other computer's name. */
  computer: string;
  /** Every Space on that computer. */
  spaces: SpaceResources[];
  /** Its base image's system ("Ubuntu 24.04"), when known. */
  baseName: string | null;
}

/** "Ubuntu 24.04.5 LTS" is "Ubuntu 24.04"; "macOS 26.5.2 (25F84)" is "macOS 26.5". */
export function systemName(pretty: string | null | undefined): string | null {
  const m = pretty?.trim().match(/^(.*?\d+(?:\.\d+)?)/);
  return m?.[1] ?? null;
}

/** `ghcr.io/trycua/linux:24.04` is "linux 24.04". */
export function imageName(reference: string): string {
  const r = reference.split("@")[0] ?? reference;
  return r.slice(r.lastIndexOf("/") + 1).replace(":", " ");
}

const ownBytes = (s: SpaceResources): number | null => s.diskOnHost?.bytes ?? null;

/** "How it fits on this Mac": each piece once (a shared base too), adding up to the volume. */
export function diskFit(host: ComputerResources, me: SpaceResources, name: string, spaces: SpaceResources[], baseName: string | null): DiskFitView {
  const total = host.diskTotal;
  const free = Math.min(host.diskFree, total);
  const used = total - free;
  const mine = ownBytes(me);
  const others = spaces.filter((s) => s.id !== me.id).reduce((n, s) => n + (ownBytes(s) ?? 0), 0);
  const bases: SharedBase[] = [];
  for (const s of [me, ...spaces.filter((o) => o.id !== me.id)]) {
    if (s.base && !bases.some((b) => b.key === s.base!.key)) bases.push(s.base);
  }
  const knownBases = bases.reduce((n, b) => n + (b.bytes ?? 0), 0);
  const counted = (mine ?? 0) + others + knownBases;
  const yours = Math.max(0, used - counted);
  const whole = Math.max(total, counted + yours + free, 1);
  const part = (key: string, kind: DiskFitKind, label: string, detail: string | null, bytes: number): DiskFitPart => ({
    key,
    kind,
    label,
    value: size(bytes),
    detail,
    bytes,
    fraction: bytes / whole,
  });
  const parts = [part("yours", "yours", "Your own files and apps", null, yours)];
  if (mine != null) parts.push(part("space", "thisSpace", name, null, mine));
  if (others > 0) parts.push(part("others", "otherSpaces", "Other Spaces", null, others));
  let note = mine == null ? `Can’t tell ${name}’s own data apart from the image it shares.` : null;
  for (const b of bases) {
    const isMine = me.base?.key === b.key;
    const label = `${isMine && baseName ? baseName : imageName(b.image)} image`;
    const detail = isMine
      ? b.sharedWith <= 1
        ? "only this Space uses it"
        : `shared by ${b.sharedWith} Spaces`
      : `used by ${b.sharedWith} Space${b.sharedWith === 1 ? "" : "s"}`;
    if (b.bytes != null) parts.push(part(b.key, "base", label, detail, b.bytes));
    else if (isMine && note == null) note = `Can’t tell how much the shared ${label} takes.`;
  }
  parts.push(part("free", "free", "Free", null, free));
  const level = host.warning.level;
  const warning =
    level === "ok"
      ? null
      : {
          text:
            level === "critical"
              ? "This Mac is out of space, so new Spaces can’t be created. Delete old Spaces or snapshots."
              : "This Mac is almost out of space. Delete old Spaces or snapshots.",
          button: "See what’s using space",
          critical: level === "critical",
        };
  return { total: size(total), parts, note, warning };
}

/** A Space's resources dialog: three cards (no explanatory sentences) and how it fits. */
export function spacePanel(i: SpacePanelInput): SpacePanelView {
  const s = i.space;
  const u = s.powerState === "running" ? s.usage : null;
  const whose = `${i.computer || "this Mac"}’s`;
  const host = i.host;
  const processor: SpacePanelCard = {
    label: LABELS.cpu,
    value: u ? `${Math.min(100, Math.max(0, Math.round(u.cpuPercent)))}% busy` : "Stopped",
    given: s.cpuAllocated ? (host?.cpuCount ? `Given ${s.cpuAllocated} of ${whose} ${cores(host.cpuCount)}` : `Given ${cores(s.cpuAllocated)}`) : null,
    fraction: u ? fill(u.cpuPercent, 100) : 0,
  };
  const memTotal = s.memoryAllocated || (u?.memoryLimited ? u.memoryTotal : 0);
  const memory: SpacePanelCard = {
    label: LABELS.memory,
    value: u ? (memTotal ? sizeOf(u.memoryUsed, memTotal) : size(u.memoryUsed)) : "Off",
    given: s.memoryAllocated
      ? host?.memoryTotal
        ? `Given ${size(s.memoryAllocated)} of ${whose} ${size(host.memoryTotal)}`
        : `Given ${size(s.memoryAllocated)}`
      : null,
    fraction: u ? fill(u.memoryUsed, memTotal) : 0,
  };
  const given = knownSize(s.diskAllocated) ?? (u?.diskLimited ? knownSize(u.diskTotal) : null);
  // Not measured on its computer: what the guest holds, only when that is its own data alone
  // (an overlay with no size of its own; a VM's guest counts its base system too).
  const guestOwn = u && !u.diskLimited && knownSize(u.diskTotal) == null && u.diskUsed < SIZE_SENTINEL_FLOOR ? u.diskUsed : null;
  const own = s.diskOnHost?.bytes ?? guestOwn;
  const files: SpacePanelCard = {
    label: LABELS.disk,
    value: own != null && given != null ? sizeOf(own, given) : own != null ? size(own) : given != null ? `Up to ${size(given)}` : "—",
    given: null,
    fraction: own != null && given != null ? fill(own, given) : null,
  };
  return {
    cards: [processor, memory, files],
    fit: host && host.diskTotal > 0 ? diskFit(host, s, i.name, i.spaces, i.baseName) : null,
  };
}
