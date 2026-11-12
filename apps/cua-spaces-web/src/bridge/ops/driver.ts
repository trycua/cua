// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Cua Driver inside Cua Spaces (CUA-2706): the driver this Mac's Cua Spaces
 * carries in `Contents/Helpers`, which `cua-spacesd` runs as its own child
 * (`embedded_driver.rs`), so it uses Cua Spaces' permissions. The SwiftUI
 * app reads it over the driver's socket (`WebUIBridge+Driver.swift`).
 *
 * | Operation | SwiftUI | Tauri |
 * |---|---|---|
 * | `driver.status`, `driver.list` | the host methods of the same names | unsupported |
 * | `driver.config`, `driver.setConfig` | `get_config` / `set_config` on the driver | unsupported |
 * | `driver.test`, `driver.pause`, `driver.setup`, `driver.setApp` | the host methods of the same names | unsupported |
 * | `driver.update` | fails by design: the driver updates with Cua Spaces | unsupported |
 * | `driver.permissions` | the host method of the same name | unsupported |
 *
 * The Electron shell (Windows, Linux) runs the driver in-process in
 * `cua-spacesd` and routes none of these methods: its "unimplemented"
 * reads as "not here", so the page doesn't show there.
 *
 * `driver.status` answers null when this Cua Spaces carries no driver; the
 * page and its sidebar entry then don't show. A field the host can't answer
 * is null or absent, and the page hides the part that needs it.
 *
 * `driver.pause` is "Let agents use apps on this Mac" turned off: the host
 * writes `<CuaHome>/run/cua-driver.off` and `cua-spacesd` stops the driver
 * until it is gone. `driver.setup` (Set up and Repair) writes the embedded
 * proxy (`cua-driver mcp --embedded --socket …`) into Claude Code's and
 * Codex's MCP configs, starts a driver whose permission answer is stale
 * again, runs the live test, and answers the new status.
 *
 * `driver.permissions` is Cua Spaces' own Screen Recording and Accessibility,
 * which the driver uses: answered on any Mac, with or without a driver (the
 * first run's permissions step, the Driver page, This Mac). Once both are
 * on, the host starts a driver whose answer is stale again, once. The
 * Electron shell answers null: no step, no rows.
 *
 * Settings are the driver's own `get_config` JSON as-is (`driver.config`);
 * `driver.setConfig` takes `set_config` keys (`cursor.float`,
 * `cursor.marks`, `cursor.rotate`, `cursor.motion.style`, `cursor.badges`).
 * `agent_cursor.enabled` is the one key that isn't `set_config`'s: the host
 * turns it into `set_agent_cursor_enabled`.
 */

import { HostError } from "../adapter";
import type { OpCoverage } from "../coverage";
import type { WebkitMethod } from "../webkit-protocol";

/** One of the two switches in System Settings, as the driver sees them (Cua Spaces' grants). */
export interface DriverPermission {
  /** `screen` (Screen Recording) or `accessibility`. */
  id: "screen" | "accessibility";
  granted: boolean;
  /**
   * Where the switch stands in System Settings: `on`; `off` (the app is in the list, switched off); `absent` (not in
   * the list yet); `unknown` (macOS doesn't say). Absent from older hosts.
   */
  state?: "on" | "off" | "absent" | "unknown" | null;
  /** The System Settings pane (`x-apple.systempreferences:`); `host.openSettings` guides it. */
  settingsUrl: string | null;
}

/** Cua Spaces' own permissions (`driver.permissions`), which Cua Driver uses. */
export interface AppPermissions {
  /** The name System Settings lists the app under: "Cua Spaces", or "Cua Spaces Local" (its own grants). */
  appName: string;
  permissions: DriverPermission[];
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
  /** The app the permissions are given to ("Cua Spaces Local"); absent from older hosts. */
  appName?: string | null;
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
  /** Connects one coding agent (`on: true`); turning one off is that agent's own setting. */
  "driver.setApp": { args: { app: string; on: boolean }; result: null };
  /** Set up and Repair: the agents' MCP entries, a fresh driver, the live test. */
  "driver.setup": { args: Record<string, never>; result: DriverStatus | null };
  "driver.update": { args: Machine; result: null };
  /** Cua Spaces' own Screen Recording and Accessibility; null where the app has none to give (Windows, Linux). */
  "driver.permissions": { args: Record<string, never>; result: AppPermissions | null };
}

export const DRIVER_OPERATIONS = [
  "driver.status",
  "driver.list",
  "driver.config",
  "driver.setConfig",
  "driver.test",
  "driver.pause",
  "driver.setApp",
  "driver.setup",
  "driver.update",
  "driver.permissions",
] as const satisfies readonly (keyof DriverOps)[];

const TAURI = "the Tauri shell is not the shipped app and runs no embedded driver";
const UPDATE = "the driver updates with Cua Spaces (a new pin in cua-driver.release.json), so the page never offers its own update";

const viaHost = (op: keyof DriverOps): OpCoverage =>
  op === "driver.update"
    ? { webkit: { methods: [], byDesign: UPDATE }, tauri: [], unsupported: { tauri: TAURI } }
    : { webkit: { methods: [op as WebkitMethod] }, tauri: [], unsupported: { tauri: TAURI } };

export const DRIVER_COVERAGE = Object.fromEntries(DRIVER_OPERATIONS.map((op) => [op, viaHost(op)])) as Record<keyof DriverOps, OpCoverage>;

/** The SwiftUI app's methods (`WebUIBridge+Driver.swift`). */
export function webkitDriverOps(request: <T>(method: WebkitMethod, args?: Record<string, unknown>) => Promise<T>): {
  [K in keyof DriverOps]: (args: DriverOps[K]["args"]) => Promise<DriverOps[K]["result"]>;
} {
  const machine = (a: Machine) => (a.machine ? { machine: a.machine } : {});
  return {
    "driver.status": (a) => request<DriverStatus | null>("driver.status", machine(a)),
    "driver.list": () => request<DriverMachine[]>("driver.list"),
    "driver.config": (a) => request<DriverConfig>("driver.config", machine(a)),
    "driver.setConfig": (a) => request<DriverConfig>("driver.setConfig", { ...machine(a), changes: a.changes }),
    "driver.test": (a) => request<DriverLiveTest>("driver.test", machine(a)),
    "driver.pause": (a) => request<null>("driver.pause", { ...machine(a), paused: a.paused }),
    "driver.setApp": (a) => request<null>("driver.setApp", { app: a.app, on: a.on }),
    "driver.setup": () => request<DriverStatus | null>("driver.setup"),
    "driver.update": () => Promise.reject(new HostError(UPDATE, "by_design")),
    "driver.permissions": () => request<AppPermissions | null>("driver.permissions"),
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
  "driver.setup": {},
  "driver.update": {},
  "driver.permissions": {},
};
