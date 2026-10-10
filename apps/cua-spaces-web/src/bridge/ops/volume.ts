// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Cua Volume's operations: the Volume page, the first run's Cua Volume
 * page and the background computer-use (cua-driver) card, plus reading
 * Settings, Experiments (the page shows only with `cuaVolume` on).
 *
 * Every operation is a daemon Spaces tool the native app already calls
 * (`PersistentModel.sendDrive`, `OnboardingModel.checkDrive`) or a shell
 * step it already runs (show in Finder, `cua agents setup --cua-driver`).
 * Nothing here is a new mechanism. Shared files register these with one
 * line each: `protocol.ts` (`OPERATIONS`), `coverage.ts` and the adapters.
 */

import { UnsupportedOperationError } from "../adapter";
import type {
  AgentSetupOutcome,
  DriveCheckInput,
  DriveGrantInput,
  DriveMountInput,
  DriveRequestInput,
  DriveStorageInput,
  DriveStorageUpdate,
  DriveSyncInput,
  Experiments,
  VolumeOverview,
} from "../contracts/volume";
import { NO_EXPERIMENTS } from "../contracts/volume";
import type { SpaceOs } from "../contracts/spaces";
import type { HostWindow } from "../detect";
import type { OpArgs, OpResult } from "../protocol";

type None = Record<string, never>;

declare module "../protocol" {
  interface HostOperations {
    /** The Volume page in one read: `volume_requests`, `volume_grants`,
     * `volume_mount_status`, `volume_sync_status`, plus the OS and home. */
    "volume.overview": { args: None; result: VolumeOverview };
    /** `volume_storage` (null when the daemon cannot answer). */
    "volume.storage": { args: None; result: DriveStorageInput | null };
    /** `volume_storage_set` with the core's update (a test is `dry_run`). */
    "volume.storageSet": { args: { update: DriveStorageUpdate }; result: DriveCheckInput };
    /** `volume_mount` / `volume_unmount`: the status after it. */
    "volume.mount": { args: None; result: DriveMountInput };
    "volume.unmount": { args: None; result: DriveMountInput };
    /** `volume_approve` (the daemon asks for presence), `volume_deny`, `volume_revoke`. */
    "volume.approve": { args: { id: string }; result: null };
    "volume.deny": { args: { id: string }; result: null };
    "volume.revoke": { args: { id: string }; result: null };
    /** `volume_sync_resolve`: the conflict copy stays, the conflict clears. */
    "volume.resolve": { args: { path: string }; result: null };
    /** Shows a path inside the mounted volume in the file manager (the shell's own call). */
    "volume.reveal": { args: { path: string }; result: null };
    /** The cua-driver skill and MCP server for `agents` (`cua agents setup --cua-driver`). */
    "agents.setupDriver": { args: { agents: string[] }; result: AgentSetupOutcome[] };
  }
}

export const VOLUME_OPERATIONS = [
  "volume.overview",
  "volume.storage",
  "volume.storageSet",
  "volume.mount",
  "volume.unmount",
  "volume.approve",
  "volume.deny",
  "volume.revoke",
  "volume.resolve",
  "volume.reveal",
  "agents.setupDriver",
] as const;

export type VolumeOp = (typeof VOLUME_OPERATIONS)[number];

/** The daemon tool each operation calls (Tauri through `agents_tool`; the
 * Swift host's `PersistentModel` and `OnboardingModel` call the same). */
export const VOLUME_TOOLS = {
  "volume.storage": "volume_storage",
  "volume.storageSet": "volume_storage_set",
  "volume.mount": "volume_mount",
  "volume.unmount": "volume_unmount",
  "volume.approve": "volume_approve",
  "volume.deny": "volume_deny",
  "volume.revoke": "volume_revoke",
  "volume.resolve": "volume_sync_resolve",
} as const;

/** `coverage.ts` rows. The SwiftUI host routes each under its own name (`ops/webkit-pages.ts`). */
export const VOLUME_COVERAGE = {
  "volume.overview": {
    webkit: { methods: ["volume.overview"] },
    tauri: ["agents_tool", "get_environment"],
  },
  "volume.storage": { webkit: { methods: ["volume.storage"] }, tauri: ["agents_tool"] },
  "volume.storageSet": { webkit: { methods: ["volume.storageSet"] }, tauri: ["agents_tool"] },
  "volume.mount": { webkit: { methods: ["volume.mount"] }, tauri: ["agents_tool"] },
  "volume.unmount": { webkit: { methods: ["volume.unmount"] }, tauri: ["agents_tool"] },
  "volume.approve": { webkit: { methods: ["volume.approve"] }, tauri: ["agents_tool"] },
  "volume.deny": { webkit: { methods: ["volume.deny"] }, tauri: ["agents_tool"] },
  "volume.revoke": { webkit: { methods: ["volume.revoke"] }, tauri: ["agents_tool"] },
  "volume.resolve": { webkit: { methods: ["volume.resolve"] }, tauri: ["agents_tool"] },
  "volume.reveal": { webkit: { methods: ["volume.reveal"] }, tauri: ["drive_reveal"] },
  "agents.setupDriver": {
    webkit: { methods: ["agents.setupDriver"] },
    tauri: [],
    unsupported: { tauri: "the Tauri shell has no cua-driver setup command (agent_setup_configure sets up skills and MCP only)" },
  },
} as const;

type Handlers = { [K in VolumeOp]: (args: OpArgs<K>) => Promise<OpResult<K>> };

/* ---- WebKit ------------------------------------------------------------------- */

/* ---- Tauri -------------------------------------------------------------------- */

type Invoke = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;

const object = <T>(v: unknown): T | null => (v !== null && typeof v === "object" && !Array.isArray(v) ? (v as T) : null);
const list = <T>(v: unknown, key: string): T[] => {
  const a = object<Record<string, unknown>>(v)?.[key];
  return Array.isArray(a) ? (a as T[]) : [];
};

/** Settings, Experiments as stored in the Tauri app's UI storage; missing keys read as off. */
export function readExperiments(raw: string | undefined): Experiments {
  try {
    const stored = raw ? (JSON.parse(raw) as Partial<Record<keyof Experiments, unknown>>) : {};
    return {
      cuaVolume: stored.cuaVolume === true,
      yourCloud: stored.yourCloud === true,
      sharing: stored.sharing === true,
      webUi: stored.webUi === true,
    };
  } catch {
    return NO_EXPERIMENTS;
  }
}

/** Each operation through `agents_tool` (the daemon tools the Tauri shell
 * allows), `drive_reveal` and the UI storage. */
export function tauriVolumeOps(invoke: Invoke, win: HostWindow): Handlers {
  const tool = <T>(name: string, args: Record<string, unknown> = {}) => invoke<T>("agents_tool", { tool: name, args });
  const maybe = <T>(name: string) => tool<unknown>(name).then(object<T>, () => null);
  const done = async (p: Promise<unknown>) => {
    await p;
    return null;
  };
  const home = (win as { __TAURI__?: { path?: { homeDir?: () => Promise<string> } } }).__TAURI__?.path?.homeDir;
  return {
    "volume.overview": async () => {
      const [requests, grants, mount, sync, env, homeDir] = await Promise.all([
        tool<unknown>("volume_requests").catch(() => null),
        tool<unknown>("volume_grants").catch(() => null),
        maybe<DriveMountInput>("volume_mount_status"),
        maybe<DriveSyncInput>("volume_sync_status"),
        invoke<{ platform?: string } | null>("get_environment").catch(() => null),
        home ? home().catch(() => null) : Promise.resolve(null),
      ]);
      const platform = env?.platform ?? "macos";
      const os: SpaceOs = platform === "linux" ? "linux" : platform === "windows" ? "windows" : "macos";
      return {
        os,
        home: homeDir,
        requests: list<DriveRequestInput>(requests, "requests"),
        grants: list<DriveGrantInput>(grants, "grants"),
        mount,
        sync,
      };
    },
    "volume.storage": () => maybe<DriveStorageInput>("volume_storage"),
    "volume.storageSet": ({ update }) => tool<DriveCheckInput>("volume_storage_set", { ...update }),
    "volume.mount": () => tool<DriveMountInput>("volume_mount"),
    "volume.unmount": () => tool<DriveMountInput>("volume_unmount"),
    "volume.approve": ({ id }) => done(tool("volume_approve", { request_id: id })),
    "volume.deny": ({ id }) => done(tool("volume_deny", { request_id: id })),
    "volume.revoke": ({ id }) => done(tool("volume_revoke", { grant_id: id })),
    "volume.resolve": ({ path }) => done(tool("volume_sync_resolve", { path })),
    "volume.reveal": ({ path }) => done(invoke("drive_reveal", { path })),
    "agents.setupDriver": () => Promise.reject(new UnsupportedOperationError("tauri", "agents.setupDriver")),
  };
}
