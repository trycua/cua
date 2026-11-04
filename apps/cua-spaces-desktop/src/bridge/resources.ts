// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// `resources.get` (CUA-1880, CUA-2204): processor, memory and disk from the
// SDK's `Spaces.resources` (the daemon's one sampler): this computer, each
// of its Spaces, your other computers' summaries, and what your Spaces on
// those computers take there. Until the SDK answers (it is starting, or the
// daemon is down), this computer's own numbers, read by the app itself as
// the SwiftUI host's `HostResourceSampler` does: busy is the share of
// non-idle processor time since the last read; disk is the volume holding
// the home folder (where Spaces live). Numbers only: never file names,
// paths or process names.
import { statfsSync } from "node:fs";
import * as os from "node:os";
import type {
  ComputerResources as SdkComputer,
  DiskWarning as SdkWarning,
  ResourcesSnapshot as SdkSnapshot,
  SpaceResources as SdkSpace,
} from "../native/generated/index";
import type { BridgeContext } from "./context";
import type { Handlers } from "./host";

interface Ticks {
  busy: number;
  total: number;
}

function ticks(): Ticks {
  let busy = 0;
  let total = 0;
  for (const c of os.cpus()) {
    const t = c.times;
    busy += t.user + t.nice + t.sys + t.irq;
    total += t.user + t.nice + t.sys + t.irq + t.idle;
  }
  return { busy, total };
}

/** Free and total bytes on the volume holding `dir`; zeros when it can't be read. */
export function diskOf(dir: string): { totalBytes: number; freeBytes: number } {
  try {
    const s = statfsSync(dir);
    return { totalBytes: s.blocks * s.bsize, freeBytes: s.bavail * s.bsize };
  } catch {
    return { totalBytes: 0, freeBytes: 0 };
  }
}

const GB = 1_000_000_000;
const GIB = 1024 ** 3;

/** The daemon's warning rule: `low` under 50 GB or 15% free, whichever line
 * is higher, capped at 100 GB; `critical` under 5 GiB. */
export function diskWarning(free: number, total: number) {
  if (total <= 0) return { level: "ok" as const, free, total, threshold: 0 };
  if (free < 5 * GIB) return { level: "critical" as const, free, total, threshold: 5 * GIB };
  const line = Math.min(Math.max(50 * GB, total * 0.15), 100 * GB);
  return free < line ? { level: "low" as const, free, total, threshold: line } : { level: "ok" as const, free, total, threshold: 0 };
}

const num = (v: bigint | number) => Number(v);

function warningJson(w: SdkWarning) {
  return { level: w.level as string as "ok" | "low" | "critical", free: num(w.free), total: num(w.total), threshold: num(w.threshold) };
}

function computerJson(c: SdkComputer) {
  return {
    id: c.id,
    name: c.name,
    online: c.online,
    sampledAtMs: num(c.sampledAtMs),
    cpuCount: c.cpuCount,
    cpuPercent: c.cpuPercent,
    loadAverage1m: c.loadAverage1m,
    memoryTotal: num(c.memoryTotal),
    memoryUsed: num(c.memoryUsed),
    diskTotal: num(c.diskTotal),
    diskFree: num(c.diskFree),
    spacesCount: c.spacesCount,
    spacesRunning: c.spacesRunning,
    spacesCpuAllocated: c.spacesCpuAllocated,
    spacesMemoryAllocated: num(c.spacesMemoryAllocated),
    spacesMemoryUsed: num(c.spacesMemoryUsed),
    spacesDiskBytes: num(c.spacesDiskBytes),
    spacesDiskExact: c.spacesDiskExact,
    warning: warningJson(c.warning),
  };
}

function spaceJson(s: SdkSpace) {
  const u = s.usage;
  const d = s.diskOnHost;
  return {
    id: s.id,
    powerState: s.powerState,
    cpuAllocated: s.cpuAllocated,
    memoryAllocated: num(s.memoryAllocated),
    diskAllocated: num(s.diskAllocated),
    usage: u
      ? {
          cpuCount: u.cpuCount,
          cpuPercent: u.cpuPercent,
          loadAverage1m: u.loadAverage1m,
          memoryUsed: num(u.memoryUsed),
          memoryTotal: num(u.memoryTotal),
          memoryLimited: u.memoryLimited,
          diskUsed: num(u.diskUsed),
          // Absent: no size of its own. Never a sentinel (the SDK says none).
          diskTotal: u.diskTotal === undefined || u.diskTotal === null ? null : num(u.diskTotal),
          diskLimited: u.diskLimited,
        }
      : null,
    diskOnHost: d ? { bytes: d.bytes === undefined ? null : num(d.bytes), allocated: num(d.allocated), sharesSpace: d.sharesSpace } : null,
    base: s.base
      ? { key: s.base.key, image: s.base.image, bytes: s.base.bytes === undefined || s.base.bytes === null ? null : num(s.base.bytes), sharedWith: s.base.sharedWith }
      : null,
  };
}

/** The SDK's snapshot as the page's `ResourcesSnapshot` (64-bit numbers as
 * numbers, a missing optional as null). */
export function snapshotJson(s: SdkSnapshot) {
  return {
    sequence: num(s.sequence),
    sampledAtMs: num(s.sampledAtMs),
    thisMac: computerJson(s.thisMac),
    spaces: s.spaces.map(spaceJson),
    computers: s.computers.map(computerJson),
    warning: warningJson(s.warning),
  };
}

/** `resources.get`: the contract's `ResourcesSnapshot`, from the SDK; this
 * computer only while the SDK can't answer. */
export function resourcesMethods(ctx: BridgeContext, read: { ticks: () => Ticks; now: () => number; home: () => string } = { ticks, now: Date.now, home: os.homedir }): Handlers {
  let last: Ticks | null = null;
  return {
    "resources.get": async (args) => {
      const visible = (args as { visible?: unknown } | undefined)?.visible !== false;
      const sdk = await Promise.resolve(ctx.model?.backend?.resources?.(visible)).catch(() => null);
      if (sdk) return snapshotJson(sdk);
      const now = read.ticks();
      const cpuPercent = last && now.total > last.total ? Math.min(100, Math.max(0, ((now.busy - last.busy) / (now.total - last.total)) * 100)) : 0;
      last = now;
      const memoryTotal = os.totalmem();
      const { totalBytes: diskTotal, freeBytes: diskFree } = diskOf(read.home());
      const warning = diskWarning(diskFree, diskTotal);
      const ms = read.now();
      return {
        sequence: Math.floor(ms / 10_000),
        sampledAtMs: ms,
        thisMac: {
          id: "this-mac",
          name: "",
          online: true,
          sampledAtMs: ms,
          cpuCount: os.cpus().length,
          cpuPercent,
          loadAverage1m: os.loadavg()[0] ?? 0,
          memoryTotal,
          memoryUsed: memoryTotal - os.freemem(),
          diskTotal,
          diskFree,
          spacesCount: 0,
          spacesRunning: 0,
          spacesCpuAllocated: 0,
          spacesMemoryAllocated: 0,
          spacesMemoryUsed: 0,
          spacesDiskBytes: 0,
          spacesDiskExact: true,
          warning,
        },
        spaces: [],
        computers: [],
        warning,
      };
    },
  };
}
