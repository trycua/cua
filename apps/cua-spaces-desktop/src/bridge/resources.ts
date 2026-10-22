// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// `resources.get` (CUA-1880): this computer's processor, memory and disk,
// read by the app itself, as the SwiftUI host's `HostResourceSampler` does.
// Busy is the share of non-idle processor time since the last read; disk is
// the volume holding the home folder (where Spaces live). Numbers only:
// never file names, paths or process names. The daemon's per-Space numbers
// join these once it serves its `resources` snapshot.
import { statfsSync } from "node:fs";
import * as os from "node:os";
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

/** `resources.get`: the contract's `ResourcesSnapshot` with this computer
 * only (no Space numbers until the daemon serves them). */
export function resourcesMethods(_ctx: BridgeContext, read: { ticks: () => Ticks; now: () => number; home: () => string } = { ticks, now: Date.now, home: os.homedir }): Handlers {
  let last: Ticks | null = null;
  return {
    "resources.get": async () => {
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
