// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The demo host's Cua Volume: an agent asking to read another agent's
 * outputs, one grant, the volume mounted in Finder (off on a first run),
 * two devices syncing with one conflict, this Mac's store, and the
 * cua-driver setup's outcomes. The Cua Volume experiment is on, so the page
 * shows. In the daemon's own (snake_case) shapes; all synthetic.
 */

import { HostError } from "../../adapter";
import type {
  AgentSetupOutcome,
  DriveCheckInput,
  DriveConflictInput,
  DriveGrantInput,
  DriveMountInput,
  DriveRequestInput,
  DriveStorageInput,
  DriveSyncInput,
  Experiments,
} from "../../contracts/volume";
import type { HostEvent, OpArgs, OpResult } from "../../protocol";
import type { VolumeOp } from "../../ops/volume";
import { demoMachine, MAC_DEMO_PLATFORM, type DemoPlatform } from "./platform";

export const DEMO_HOME = "/Users/ada";
const MOUNT_PATH = `${DEMO_HOME}/Cua Volume`;

export interface DemoVolumeState {
  experiments: Experiments;
  requests: DriveRequestInput[];
  grants: DriveGrantInput[];
  mounted: boolean;
  conflicts: DriveConflictInput[];
  storage: DriveStorageInput;
}

export function demoVolumeState(now: number, onboarded: boolean, platform: DemoPlatform = MAC_DEMO_PLATFORM): DemoVolumeState {
  return {
    experiments: { cuaVolume: true, yourCloud: false, sharing: false, webUi: false },
    requests: [
      { id: "req-research", principal: "agent:researcher", prefix: "agents/writer/outputs/", mode: "r", reason: "Cite the launch draft in the weekly summary" },
    ],
    grants: [{ id: "grant-writer", principal: "agent:writer", prefix: "public/", mode: "rw" }],
    mounted: onboarded,
    conflicts: [
      {
        path: "public/plan.md",
        conflict_path: "public/plan (conflict from linux-box 2026-10-03 09.41.12).md",
        winner_device: "this-mac",
        loser_device: "linux-box",
        ts_ms: now - 22 * 60_000,
      },
    ],
    storage: { backend: "fs", fs_path: platform.os === "macos" ? `${DEMO_HOME}/.cua/volume/data` : demoMachine(platform).dataPath, s3: null, has_keys: false },
  };
}

const mountStatus = (mounted: boolean, platform: DemoPlatform = MAC_DEMO_PLATFORM): DriveMountInput =>
  platform.os === "macos" ? macMountStatus(mounted) : demoMachine(platform).mount(mounted);

const macMountStatus = (mounted: boolean): DriveMountInput => ({
  enabled: mounted,
  state: mounted ? "mounted" : "off",
  method: "fskit",
  path: mounted ? MOUNT_PATH : null,
  volume_name: "Cua Volume",
});

function syncStatus(s: DemoVolumeState, now: number, here: string): DriveSyncInput {
  return {
    device_id: "this-mac",
    device_name: here,
    feed: "live",
    last_poll_ms: now - 4_000,
    pending_uploads: s.conflicts.length ? 2 : 0,
    conflicts: s.conflicts,
    devices: [
      { id: "this-mac", name: here, this_device: true, last_seen_ms: now - 4_000, last_change_ms: now - 60_000 },
      { id: "linux-box", name: "Linux box", this_device: false, last_seen_ms: now - 6 * 60_000, last_change_ms: now - 22 * 60_000 },
    ],
    last_error: null,
  };
}

/** What `cua agents setup --cua-driver` reports for each agent. */
export function demoDriverOutcomes(agents: string[]): AgentSetupOutcome[] {
  return agents.flatMap((a) => [
    { agents: [a], target: "skill", item: "cua-driver", change: "created", detail: "" },
    { agents: [a], target: "mcp", item: "cua-driver", change: "created", detail: "" },
  ]);
}

interface DemoContext {
  state: DemoVolumeState;
  now: () => number;
  wait: (ms: number) => Promise<void>;
  step: number;
  emit: (e: HostEvent) => void;
  /** The machine the demo runs on (a Mac unless the shell says). */
  platform?: DemoPlatform;
}

/** The demo adapter's handlers for `ops/volume.ts`. */
export function demoVolumeHandlers(ctx: DemoContext): { [K in VolumeOp]: (args: OpArgs<K>) => Promise<OpResult<K>> | OpResult<K> } {
  const { state: s, now, wait, step } = ctx;
  const platform = ctx.platform ?? MAC_DEMO_PLATFORM;
  const machine = demoMachine(platform);
  const home = platform.os === "macos" ? DEMO_HOME : machine.home;
  const mountPath = platform.os === "macos" ? MOUNT_PATH : (machine.mount(true).path ?? null);
  return {
    "volume.overview": () => ({
      os: platform.os,
      home,
      requests: s.requests.map((r) => ({ ...r })),
      grants: s.grants.map((g) => ({ ...g })),
      mount: mountStatus(s.mounted, platform),
      sync: syncStatus(s, now(), machine.name),
    }),
    "volume.storage": () => ({ ...s.storage }),
    "volume.storageSet": async ({ update }): Promise<DriveCheckInput> => {
      await wait(step);
      const bucket = update.s3?.bucket ?? "";
      if (update.backend === "s3" && !bucket) {
        return { ok: false, reachable: false, authorized: false, versioning: false, detail: "Enter a bucket name.", applied: false };
      }
      const applied = !update.dry_run;
      if (applied) s.storage = { ...s.storage, backend: update.backend, s3: update.s3, has_keys: Boolean(update.access_key_id) || s.storage.has_keys };
      return { ok: true, reachable: true, authorized: true, versioning: true, detail: null, applied };
    },
    "volume.mount": async () => {
      await wait(step);
      s.mounted = true;
      return mountStatus(true, platform);
    },
    "volume.unmount": async () => {
      await wait(step);
      s.mounted = false;
      return mountStatus(false, platform);
    },
    "volume.approve": async ({ id }) => {
      const r = s.requests.find((q) => q.id === id);
      if (!r) throw new HostError(`no request ${id}`, "not_found");
      await wait(step);
      s.requests = s.requests.filter((q) => q.id !== id);
      s.grants = [...s.grants, { id: `grant-${id}`, principal: r.principal, prefix: r.prefix, mode: r.mode }];
      return null;
    },
    "volume.deny": ({ id }) => {
      s.requests = s.requests.filter((q) => q.id !== id);
      return null;
    },
    "volume.revoke": ({ id }) => {
      s.grants = s.grants.filter((g) => g.id !== id);
      return null;
    },
    "volume.resolve": ({ path }) => {
      s.conflicts = s.conflicts.filter((c) => c.path !== path);
      return null;
    },
    // There is no Finder in the browser; the path only has to be inside the volume.
    "volume.reveal": ({ path }) => {
      if (!s.mounted || !mountPath || !path.startsWith(mountPath)) throw new HostError("That path is not in the mounted volume.", "not_found");
      return null;
    },
    "agents.setupDriver": async ({ agents }) => {
      await wait(step);
      return demoDriverOutcomes(agents);
    },
  };
}
