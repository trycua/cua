// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Cua Driver inside Cua Spaces (CUA-2706): the driver `cua-spacesd` runs
 * for coding agents on this Mac (`embedded_driver.rs`, trycua/cua#6145),
 * managed from the app's Cua Driver page, the first run and the top bar.
 *
 * | Operation | SwiftUI / Electron | Tauri |
 * |---|---|---|
 * | `driver.status` | not yet (CUA-2775 API) | unsupported |
 * | `driver.list` | not yet | unsupported |
 * | `driver.config`, `driver.setConfig` | not yet | unsupported |
 * | `driver.test`, `driver.pause`, `driver.setApp`, `driver.update` | not yet | unsupported |
 *
 * The native hosts answer these once `cua-spacesd` exposes its
 * `EmbeddedDriver` through the app API (`status()`, `get_config()`,
 * `set_config()`, `permissions()`, `live_test()`). Until then every host
 * but the demo rejects with `unsupported`, and the UI draws nothing about
 * the driver. The page also stays hidden unless the `spaces_embedded_driver`
 * flag is on (the UI's side of `CUA_SPACESD_EMBEDDED_DRIVER=1`).
 *
 * `driver.status` answers null when the host runs no embedded driver (the
 * service is off, or this isn't a Mac). A field the backend can't answer is
 * null or absent, and the page hides the part that needs it.
 *
 * Settings are the driver's own `get_config` JSON as-is (`driver.config`);
 * `driver.setConfig` takes `set_config` keys (`cursor.float`,
 * `cursor.marks`, `cursor.rotate`, `cursor.motion.style`, `cursor.badges`).
 * `agent_cursor.enabled` is the one key that isn't `set_config`'s: the host
 * turns it into `set_agent_cursor_enabled`.
 */

import { UnsupportedOperationError, type BridgeMode } from "../adapter";
import type { OpCoverage } from "../coverage";

/** One of the two switches in System Settings, as the driver sees them (Cua Spaces' grants). */
export interface DriverPermission {
  /** `screen` (Screen Recording) or `accessibility`. */
  id: "screen" | "accessibility";
  granted: boolean;
  /** The System Settings pane (`x-apple.systempreferences:`); `host.openSettings` guides it. */
  settingsUrl: string | null;
}

/** `embedded_driver::LiveTest`: both permissions, the screen read, and one agent-pointer move. */
export interface DriverLiveTest {
  passed: boolean;
  /** When it ran (ms since the epoch). */
  atMs: number;
  /** Which step failed, in plain words. */
  failure: string | null;
  /** How long it took (ms), when the host timed it. */
  tookMs?: number | null;
}

/** A newer driver the host has ready (CUA-2776). Absent: no Update button. */
export interface DriverUpdate {
  version: string;
  /** One plain line on what's better ("Faster clicks in Chrome"). */
  note: string | null;
}

/** A coding app the driver is registered with. */
export interface DriverApp {
  /** `claude-code`, `codex`, `t3-code`. */
  id: string;
  name: string;
  /** It uses this Cua Driver. */
  on: boolean;
  /** The person's own `cua-driver` entry, left in place: the switch is read-only. */
  own: boolean;
  /** Brings its own driver (T3 Code): nothing for Cua Spaces to register. */
  bringsOwn: boolean;
}

/** An agent using the driver right now. */
export interface DriverActivity {
  /** The coding app ("Claude Code"). */
  app: string;
  /** The app it is working in ("Xcode"). */
  target: string;
}

/** `embedded_driver::DriverStatus`, plus what the app adds around it. */
export interface DriverStatus {
  machineId: string;
  machineName: string;
  thisMachine: boolean;
  /**
   * `ready`: running, permissions on. `starting`, `stopped`. `paused`: the
   * person paused agents. `needs-permission`: a System Settings switch is
   * off. `failed`: it keeps exiting (`lastError` says why).
   */
  state: "ready" | "starting" | "stopped" | "paused" | "needs-permission" | "failed";
  version: string | null;
  update: DriverUpdate | null;
  lastTest: DriverLiveTest | null;
  lastError: string | null;
  permissions: DriverPermission[];
  /** Null: the host can't say which apps use it (the section hides). */
  apps: DriverApp[] | null;
  activity: DriverActivity[];
  /** The host can pause agents (`driver.pause`). */
  canPause: boolean;
}

/** One computer on the account with a Cua Driver, for the page's list of computers. */
export interface DriverMachine {
  machineId: string;
  name: string;
  os: "macos" | "windows" | "linux";
  thisMachine: boolean;
  state: DriverStatus["state"];
  version: string | null;
  update: string | null;
}

/** The driver's `get_config` JSON, as-is. */
export type DriverConfig = Record<string, unknown>;

/** A `set_config` value: on/off switches and named choices are strings; `agent_cursor.enabled` is a boolean. */
export type DriverConfigValue = string | boolean;

type Machine = { machine?: string | null };

export interface DriverOps {
  "driver.status": { args: Machine; result: DriverStatus | null };
  "driver.list": { args: Record<string, never>; result: DriverMachine[] };
  "driver.config": { args: Machine; result: DriverConfig };
  "driver.setConfig": { args: Machine & { changes: Record<string, DriverConfigValue> }; result: DriverConfig };
  "driver.test": { args: Machine; result: DriverLiveTest };
  "driver.pause": { args: Machine & { paused: boolean }; result: null };
  "driver.setApp": { args: { app: string; on: boolean }; result: null };
  "driver.update": { args: Machine; result: null };
}

export const DRIVER_OPERATIONS = [
  "driver.status",
  "driver.list",
  "driver.config",
  "driver.setConfig",
  "driver.test",
  "driver.pause",
  "driver.setApp",
  "driver.update",
] as const satisfies readonly (keyof DriverOps)[];

const NOT_YET = "cua-spacesd's embedded driver (trycua/cua#6145) reaches the app API in the next PR (CUA-2775); until then the page stays hidden";
const TAURI = "the Tauri shell is not the shipped app and runs no embedded driver";

export const DRIVER_COVERAGE = Object.fromEntries(
  DRIVER_OPERATIONS.map((op) => [op, { webkit: { methods: [] }, tauri: [], unsupported: { webkit: NOT_YET, tauri: TAURI } }]),
) as unknown as Record<keyof DriverOps, OpCoverage>;

/** Hosts with no embedded driver yet. */
export function unsupportedDriverOps(mode: BridgeMode): { [K in keyof DriverOps]: (args: DriverOps[K]["args"]) => Promise<DriverOps[K]["result"]> } {
  const no = (op: keyof DriverOps) => () => Promise.reject(new UnsupportedOperationError(mode, op));
  return {
    "driver.status": no("driver.status"),
    "driver.list": no("driver.list"),
    "driver.config": no("driver.config"),
    "driver.setConfig": no("driver.setConfig"),
    "driver.test": no("driver.test"),
    "driver.pause": no("driver.pause"),
    "driver.setApp": no("driver.setApp"),
    "driver.update": no("driver.update"),
  };
}

/** Arguments that take each down its usual path (coverage and contract tests). */
export const DRIVER_ARGS = {
  "driver.status": {},
  "driver.list": {},
  "driver.config": {},
  "driver.setConfig": { changes: { "cursor.float": "off" } },
  "driver.test": {},
  "driver.pause": { paused: false },
  "driver.setApp": { app: "codex", on: true },
  "driver.update": {},
};
