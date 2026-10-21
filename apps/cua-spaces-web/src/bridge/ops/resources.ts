// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Processor, memory and disk for each computer and each extra Space
 * (CUA-1880, `2026-10-18-space-resources-spec.md`, "API (as built)"): the
 * SDK's `Spaces.resources(visible)` snapshot, as the host answers it.
 *
 * The local daemon samples this Mac and every local Space. `visible: true`
 * counts as "a window is open" for 30 s, so the daemon samples every 10 s
 * then and every 5 minutes otherwise. Other computers publish a summary
 * through the relay machine record; `sampledAtMs` says how old it is.
 *
 * | Operation | Electron | SwiftUI | Tauri |
 * |---|---|---|---|
 * | `resources.get {visible}` | the host method `resources.get` | the host method `resources.get` | unsupported |
 *
 * Null, or a host that doesn't answer it yet: the app shows no numbers.
 * Numbers only: never file names, paths, process names or window titles.
 */

import type { OpCoverage } from "../coverage";

/**
 * The disk warning (`cua_spaces::resources::DiskWarning`). `low`: free is
 * under 50 GB or 15% of the volume, whichever line is higher, capped at
 * 100 GB. `critical`: under 5 GiB, where creates already fail. It clears
 * 2 GiB above the line, so it doesn't flicker.
 */
export interface DiskWarning {
  level: "ok" | "low" | "critical";
  free: number;
  total: number;
  /** The line that was crossed (0 when ok). */
  threshold: number;
}

/** One computer (`ComputerResources`): this Mac, or another computer's relay summary. */
export interface ComputerResources {
  /** `this-mac`, or the relay machine id (`machines.list`). */
  id: string;
  /** Empty for this Mac. */
  name: string;
  online: boolean;
  /** When the computer measured it, unix ms (another computer's: up to 5 minutes old). */
  sampledAtMs: number;
  cpuCount: number;
  /** Busy now, 0 to 100. */
  cpuPercent: number;
  loadAverage1m: number;
  memoryTotal: number;
  /** Without reclaimable cache. */
  memoryUsed: number;
  /** The volume that holds Spaces. */
  diskTotal: number;
  diskFree: number;
  spacesCount: number;
  spacesRunning: number;
  /** Cores given to running Spaces. */
  spacesCpuAllocated: number;
  /** Bytes given to running Spaces. */
  spacesMemoryAllocated: number;
  spacesMemoryUsed: number;
  /** What all extra Spaces take on this computer's disk. */
  spacesDiskBytes: number;
  /** False: some Spaces could only be estimated. */
  spacesDiskExact: boolean;
  warning: DiskWarning;
}

/** What the guest reports (`SystemService.Metrics`, with CPU). */
export interface GuestUsage {
  cpuCount: number;
  cpuPercent: number;
  loadAverage1m: number;
  memoryUsed: number;
  memoryTotal: number;
  memoryLimited: boolean;
  diskUsed: number;
  diskTotal: number;
  diskLimited: boolean;
}

/** What a Space costs this Mac's disk. */
export interface DiskOnHost {
  /** Bytes only this Space holds (what deleting it frees); null: couldn't measure. */
  bytes: number | null;
  /** Blocks its files occupy, clones counted in full (an upper bound). */
  allocated: number;
  /** Part of it is shared with other copies. */
  sharesSpace: boolean;
}

/** One extra Space on this Mac (`SpaceResources`). */
export interface SpaceResources {
  /** `local:aurora`. */
  id: string;
  powerState: "running" | "stopped" | "suspended" | "";
  /** Cores; 0: unknown. */
  cpuAllocated: number;
  /** Bytes; 0: unknown. */
  memoryAllocated: number;
  /** The disk's size limit ("up to 80 GB"); 0: unknown. */
  diskAllocated: number;
  /** Null while stopped or unreachable. */
  usage: GuestUsage | null;
  /** Null: nothing to say (a container). */
  diskOnHost: DiskOnHost | null;
}

/** `cua_spaces::resources::ResourcesSnapshot` (the SDK's `Spaces.resources(visible)`). */
export interface ResourcesSnapshot {
  /** Grows by 1 per sample. */
  sequence: number;
  sampledAtMs: number;
  thisMac: ComputerResources;
  /** Extra Spaces on this Mac (local Spaces only). */
  spaces: SpaceResources[];
  /** Your other computers, from their relay summary. */
  computers: ComputerResources[];
  /** Same as `thisMac.warning`, for the menu bar. */
  warning: DiskWarning;
}

export interface ResourcesOps {
  "resources.get": { args: { visible: boolean }; result: ResourcesSnapshot | null };
}

export const RESOURCES_OPERATIONS = ["resources.get"] as const satisfies readonly (keyof ResourcesOps)[];

export const RESOURCES_COVERAGE = {
  "resources.get": {
    webkit: { methods: ["resources.get"] },
    tauri: [],
    unsupported: {
      tauri: "the Tauri app does not read resource numbers",
    },
  },
} as const satisfies Record<keyof ResourcesOps, OpCoverage>;

/** Arguments that take it down its usual path (coverage tests). */
export const RESOURCES_ARGS = { "resources.get": { visible: true } };
