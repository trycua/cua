// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The demo host's resource numbers (`resources.get`): This Mac and each of
 * its Spaces, a summary from the Mac mini (the Linux box sends none), and
 * what each of your Spaces on your other computers takes there (only its
 * disk; none for a computer that is offline).
 * Busy and in-use numbers drift a little with the clock, so the bars move
 * as live ones do. `diskFree` in its state sets This Mac's free disk
 * (`?demo=low-disk`, `?demo=disk-full`).
 */

import type { SpaceRow } from "../../contracts/spaces";
import type { ComputerResources, DiskWarning, ResourcesSnapshot, SpaceResources } from "../../ops/resources";
import type { DemoContext, DemoHandlers } from "./context";
import { DEMO_MACHINES } from "../demo-data";

const GB = 1_000_000_000;
const GIB = 1024 ** 3;

export interface DemoResourcesState {
  /** This Mac's free disk, in bytes. */
  diskFree: number;
}

export const DEMO_DISK_FREE = { healthy: 310 * GB, low: 41 * GB, full: 3.2 * GB } as const;

export const demoResourcesState = (disk: keyof typeof DEMO_DISK_FREE = "healthy"): DemoResourcesState => ({ diskFree: DEMO_DISK_FREE[disk] });

/** The daemon's warning rule (without its 2 GiB hysteresis): `low` under
 * 50 GB or 15%, whichever line is higher, capped at 100 GB; `critical`
 * under 5 GiB. */
export function demoDiskWarning(free: number, total: number): DiskWarning {
  if (free < 5 * GIB) return { level: "critical", free, total, threshold: 5 * GIB };
  const line = Math.min(Math.max(50 * GB, total * 0.15), 100 * GB);
  return free < line ? { level: "low", free, total, threshold: line } : { level: "ok", free, total, threshold: 0 };
}

/** A small, smooth wobble around `base` (±`by`), from the clock. */
const drift = (now: number, base: number, by: number, phase = 0) => Math.max(0, base + Math.sin(now / 9000 + phase) * by);

const running = (row: SpaceRow) => row.powerState === "running" || (row.powerState === undefined && row.reachable);

/** What each demo Space was given (cores, GB of memory, GB of disk at most) and takes on its host. */
const ALLOCATION: Record<string, { cores: number; memory: number; disk: number; onHost: number; inside: number; shared: boolean }> = {
  "local:design-review": { cores: 4, memory: 8, disk: 80, onHost: 19, inside: 34, shared: true },
  "local:agent-sandbox": { cores: 2, memory: 4, disk: 40, onHost: 6, inside: 6, shared: false },
};

function spaceResources(row: SpaceRow, now: number, index: number): SpaceResources {
  const a = ALLOCATION[row.id] ?? (row.os === "linux" ? { cores: 2, memory: 4, disk: 40, onHost: 6, inside: 5, shared: false } : { cores: 4, memory: 8, disk: 80, onHost: 22, inside: 30, shared: true });
  const on = running(row);
  const cpu = Math.round(drift(now, 18 + index * 4, 7, index) * 10) / 10;
  return {
    id: row.id,
    powerState: on ? "running" : "stopped",
    cpuAllocated: a.cores,
    memoryAllocated: a.memory * GB,
    diskAllocated: a.disk * GB,
    usage: on
      ? {
          cpuCount: a.cores,
          cpuPercent: cpu,
          loadAverage1m: (cpu / 100) * a.cores,
          memoryUsed: Math.round(drift(now, a.memory * 0.38, 0.2, index) * GB),
          memoryTotal: a.memory * GB,
          memoryLimited: true,
          diskUsed: a.inside * GB,
          diskTotal: a.disk * GB,
          diskLimited: true,
        }
      : null,
    diskOnHost: { bytes: a.onHost * GB, allocated: (a.shared ? a.inside : a.onHost) * GB, sharesSpace: a.shared },
  };
}

/** GB each demo Space on another computer takes there (stopped ones too). */
const ON_OTHER_COMPUTER: Record<string, number> = {
  "relay:mac-mini/release-checks": 21,
  "relay:mac-mini/qa-windows": 25,
  "relay:linux-box/ubuntu-build": 5,
  "relay:linux-box/windows-legacy": 31,
};

const POWER_STATES = ["running", "stopped", "suspended"] as const;

/** One of your Spaces on another computer: only what it takes there. */
function remoteSpaceResources(row: SpaceRow): SpaceResources {
  const gb = ON_OTHER_COMPUTER[row.id] ?? (row.kind === "container" ? 4 : 24);
  return {
    id: row.id,
    powerState: POWER_STATES.find((p) => p === row.powerState) ?? "",
    cpuAllocated: 0,
    memoryAllocated: 0,
    diskAllocated: 0,
    usage: null,
    diskOnHost: { bytes: gb * GB, allocated: gb * GB, sharesSpace: false },
  };
}

function computer(c: Omit<ComputerResources, "warning">): ComputerResources {
  return { ...c, warning: demoDiskWarning(c.diskFree, c.diskTotal) };
}

export function demoResources(rows: readonly SpaceRow[], now: number, diskFree: number = DEMO_DISK_FREE.healthy): ResourcesSnapshot {
  // Only this Mac's own Spaces: other computers send a summary, not each Space.
  const spaces = rows.filter((r) => r.provider === "local").map((r, i) => spaceResources(r, now, i));
  const on = spaces.filter((s) => s.powerState === "running");
  const sum = (list: SpaceResources[], f: (s: SpaceResources) => number | null | undefined) => list.reduce((n, s) => n + (f(s) ?? 0), 0);
  const cpuPercent = Math.round(drift(now, 34, 6) * 10) / 10;
  const thisMac = computer({
    id: "this-mac",
    name: "",
    online: true,
    sampledAtMs: now,
    cpuCount: 12,
    cpuPercent,
    loadAverage1m: (cpuPercent / 100) * 12,
    memoryTotal: 64 * GB,
    memoryUsed: Math.round(drift(now, 41, 1.5) * GB),
    diskTotal: 1000 * GB,
    diskFree,
    spacesCount: spaces.length,
    spacesRunning: on.length,
    // The demo counts stopped Spaces' allocation too, as frame 14 shows it.
    spacesCpuAllocated: sum(spaces, (s) => s.cpuAllocated),
    spacesMemoryAllocated: sum(spaces, (s) => s.memoryAllocated),
    spacesMemoryUsed: sum(on, (s) => s.usage?.memoryUsed),
    spacesDiskBytes: sum(spaces, (s) => s.diskOnHost?.bytes),
    spacesDiskExact: false,
  });
  // The Mac mini's summary through the relay, a couple of minutes old.
  const macMini = computer({
    id: "mac-mini",
    name: "Mac mini",
    online: true,
    sampledAtMs: now - 2 * 60_000,
    cpuCount: 8,
    cpuPercent: 12,
    loadAverage1m: 1,
    memoryTotal: 16 * GB,
    memoryUsed: 7 * GB,
    diskTotal: 512 * GB,
    diskFree: 212 * GB,
    spacesCount: 2,
    spacesRunning: 1,
    spacesCpuAllocated: 6,
    spacesMemoryAllocated: 12 * GB,
    spacesMemoryUsed: 5 * GB,
    spacesDiskBytes: 46 * GB,
    spacesDiskExact: false,
  });
  const online = (host: string) => DEMO_MACHINES.some((m) => m.id === host && m.online);
  const remote = rows.filter((r) => r.provider !== "local" && r.host && online(r.host)).map(remoteSpaceResources);
  return { sequence: Math.floor(now / 10_000), sampledAtMs: now, thisMac, spaces: [...spaces, ...remote], computers: [macMini], warning: thisMac.warning };
}

export function demoResourcesHandlers({ state, now }: DemoContext): DemoHandlers<"resources.get"> {
  return {
    "resources.get": () => demoResources(state.rows, now(), state.diskFree),
  };
}
