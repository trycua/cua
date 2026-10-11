// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Operations for Settings beyond General: About, Devices, Storage,
 * Experiments and launch at login. Each is an existing host call under a
 * host-neutral name (README, "Settings"); nothing here is a new mechanism.
 * `protocol.ts` registers them; the Tauri and SwiftUI mappings live here so
 * the adapters each take one line.
 */

import { UnsupportedOperationError } from "../adapter";
import type { LoginItemStatus } from "../contracts/host";
import type { DevicesInput, EnrollResult } from "../contracts/devices";
import type {
  AboutInput,
  DriveCacheInput,
  DriveCheckInput,
  DriveMountInput,
  DriveStorageInput,
  Experiments,
  LoginItemReport,
  StorageInput,
  StorageRequest,
  UpdateChannelId,
} from "../contracts/settings";

type Empty = Record<string, never>;

export interface SettingsOperations {
  /** The app's name, version and updater (`about::AboutInput`). */
  "about.get": { args: Empty; result: AboutInput };
  /** Changes an update control; answers the new input. */
  "about.set": { args: { autoCheck?: boolean; autoInstall?: boolean; channel?: UpdateChannelId }; result: AboutInput };
  /** Check Now. */
  "about.checkNow": { args: Empty; result: AboutInput };

  /** Settings, Experiments: the switches as stored. */
  "experiments.get": { args: Empty; result: Experiments };
  "experiments.set": { args: { experiments: Experiments }; result: Experiments };

  /** Launch at login as the system reports it, and what this machine serves. */
  "loginItem.get": { args: Empty; result: LoginItemReport };
  "loginItem.set": { args: { on: boolean }; result: LoginItemReport };
  /** System Settings, Login Items (to approve it). */
  "loginItem.openSettings": { args: Empty; result: null };

  /** The account's devices and access log (`devices_snapshot`). */
  "devices.get": { args: Empty; result: DevicesInput };
  /** Registers this device: enrolled at once, or a one-time code. */
  "devices.enroll": { args: Empty; result: EnrollResult };
  /** Whether an enrolled device approved this one yet. */
  "devices.checkEnrolled": { args: Empty; result: boolean };
  /** Approves a device by code or id. The host asks for presence first. */
  "devices.approve": { args: { code: string | null; deviceId: string | null }; result: null };
  "devices.rename": { args: { id: string; name: string }; result: null };
  /** Revokes a device (Deny, or Revoke… after its confirmation). */
  "devices.revoke": { args: { id: string }; result: null };
  /** Vouches for a machine that registered without an enrolled device's proof. */
  "devices.confirmMachine": { args: { id: string }; result: null };

  /** The Cua Volume's storage, mount and cache (`volume_storage`, `volume_mount_status`, `volume_cache_stats`). */
  "storage.get": { args: Empty; result: StorageInput };
  /** Runs the core's `StorageRequest`; test, save and adopt answer the check. */
  "storage.run": { args: { request: StorageRequest }; result: DriveCheckInput | null };
}

export type SettingsOpName = keyof SettingsOperations;

export const SETTINGS_OPERATIONS = [
  "about.get",
  "about.set",
  "about.checkNow",
  "experiments.get",
  "experiments.set",
  "loginItem.get",
  "loginItem.set",
  "loginItem.openSettings",
  "devices.get",
  "devices.enroll",
  "devices.checkEnrolled",
  "devices.approve",
  "devices.rename",
  "devices.revoke",
  "devices.confirmMachine",
  "storage.get",
  "storage.run",
] as const satisfies readonly SettingsOpName[];

type Handlers = { [K in SettingsOpName]: (args: SettingsOperations[K]["args"]) => Promise<SettingsOperations[K]["result"]> };
type Invoke = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;

/** A tool answer that is a JSON object, or null. */
const objectOrNull = <T>(v: unknown): T | null => (v !== null && typeof v === "object" && !Array.isArray(v) ? (v as T) : null);

const osOf = (platform: string | undefined): StorageInput["os"] =>
  platform === "linux" || platform === "windows" ? platform : "macos";

/* ---- Tauri ----------------------------------------------------------------- */

/** Tauri: the shell's commands, and the daemon's Spaces tools through `agents_tool`. */
export function tauriSettingsOps(invoke: Invoke, ui: { read(key: string): string | undefined; write(key: string, value: string): Promise<void> }): Handlers {
  const tool = (name: string, args: Record<string, unknown> = {}) => invoke<unknown>("agents_tool", { tool: name, args });
  const platform = () => invoke<{ platform?: string } | null>("get_environment").then((e) => e?.platform, () => undefined);

  const about = async (): Promise<AboutInput> => {
    const [version, p] = await Promise.all([invoke<string | null>("plugin:app|version").catch(() => null), platform()]);
    // The Tauri updater has one feed and no controls to offer.
    return { platform: osOf(p), version: version ?? "", build: "", os: "", updater: false, autoCheck: false, autoInstall: false, channel: "stable", lastCheck: null, checking: false };
  };

  const loginItem = async (): Promise<LoginItemReport> => {
    const [status, host, agents] = await Promise.all([
      invoke<LoginItemStatus>("login_item_status"),
      invoke<{ provideSpaces?: boolean } | null>("host_status").catch(() => null),
      tool("persistent_agent_list").catch(() => null),
    ]);
    const list = (agents as { agents?: unknown[] } | null)?.agents ?? [];
    return { status: status ?? "notFound", providesSpaces: Boolean(host?.provideSpaces), runsAgents: list.length > 0 };
  };

  const storage = async (): Promise<StorageInput> => {
    const [p, s, m, c] = await Promise.all([
      platform(),
      tool("volume_storage").catch(() => null),
      tool("volume_mount_status").catch(() => null),
      tool("volume_cache_stats").catch(() => null),
    ]);
    return { os: osOf(p), home: null, storage: objectOrNull<DriveStorageInput>(s), mount: objectOrNull<DriveMountInput>(m), cache: objectOrNull<DriveCacheInput>(c) };
  };

  const EXPERIMENTS_KEY = "cua.settings.experiments";
  const readExperiments = (): Experiments => {
    try {
      return (JSON.parse(ui.read(EXPERIMENTS_KEY) ?? "{}") as Experiments) ?? {};
    } catch {
      return {};
    }
  };

  return {
    "about.get": about,
    "about.set": () => Promise.reject(new UnsupportedOperationError("tauri", "about.set")),
    "about.checkNow": () => Promise.reject(new UnsupportedOperationError("tauri", "about.checkNow")),

    // The Tauri app keeps the switches in its UI storage, as its own Settings does.
    "experiments.get": async () => readExperiments(),
    "experiments.set": async ({ experiments }) => {
      await ui.write(EXPERIMENTS_KEY, JSON.stringify(experiments));
      return readExperiments();
    },

    "loginItem.get": loginItem,
    "loginItem.set": async ({ on }) => {
      await invoke("login_item_set", { on });
      return loginItem();
    },
    "loginItem.openSettings": async () => {
      await invoke("host_open_settings", { url: "x-apple.systempreferences:com.apple.LoginItems-Settings.extension" });
      return null;
    },

    "devices.get": () => invoke<DevicesInput>("devices_snapshot"),
    "devices.enroll": () => invoke<EnrollResult>("devices_enroll"),
    "devices.checkEnrolled": () => invoke<boolean>("devices_check_enrolled"),
    "devices.approve": async ({ code, deviceId }) => {
      await invoke("devices_approve", { code, deviceId, passphrase: null });
      return null;
    },
    "devices.rename": async ({ id, name }) => {
      await invoke("devices_rename", { id, name });
      return null;
    },
    "devices.revoke": async ({ id }) => {
      await invoke("devices_revoke", { id });
      return null;
    },
    "devices.confirmMachine": async ({ id }) => {
      await invoke("devices_confirm_machine", { id });
      return null;
    },

    "storage.get": storage,
    "storage.run": async ({ request }) => {
      switch (request.kind) {
        case "test":
        case "save":
        case "adopt":
          return (await tool("volume_storage_set", { ...request.update })) as DriveCheckInput;
        case "mount":
          await tool("volume_mount");
          return null;
        case "unmount":
          await tool("volume_unmount");
          return null;
        case "reveal":
          await invoke("drive_reveal", { path: request.path });
          return null;
        case "open-url":
          await invoke("host_open_settings", { url: request.url });
          return null;
        case "set-cache":
          await tool("volume_cache_set", { capacity_bytes: request.capacity_bytes });
          return null;
        case "clear-cache":
          await tool("volume_cache_clear");
          return null;
      }
    },
  };
}

/** The Tauri updater has one feed and no controls. */
export const TAURI_SETTINGS_UNSUPPORTED = ["about.set", "about.checkNow"] as const satisfies readonly SettingsOpName[];

/** The Tauri commands each operation calls (`coverage.ts`). */
export const SETTINGS_TAURI_COMMANDS = {
  "about.get": ["plugin:app|version", "get_environment"],
  "about.set": [],
  "about.checkNow": [],
  "experiments.get": [],
  "experiments.set": ["ui_storage_set"],
  "loginItem.get": ["login_item_status", "host_status", "agents_tool"],
  "loginItem.set": ["login_item_set", "login_item_status", "host_status", "agents_tool"],
  "loginItem.openSettings": ["host_open_settings"],
  "devices.get": ["devices_snapshot"],
  "devices.enroll": ["devices_enroll"],
  "devices.checkEnrolled": ["devices_check_enrolled"],
  "devices.approve": ["devices_approve"],
  "devices.rename": ["devices_rename"],
  "devices.revoke": ["devices_revoke"],
  "devices.confirmMachine": ["devices_confirm_machine"],
  "storage.get": ["get_environment", "agents_tool"],
  "storage.run": ["agents_tool", "drive_reveal", "host_open_settings"],
} as const satisfies Record<SettingsOpName, readonly string[]>;

/* ---- SwiftUI (WKWebView) --------------------------------------------------- */

/** `app.info` (the parts read here). */
interface WkAppInfo {
  platform?: string;
  version?: string;
  experiments?: Experiments | null;
}

/** The experiments' row ids (`experiments::row_id`). */
const EXPERIMENT_ROWS: Record<keyof Experiments, string> = {
  cuaVolume: "experiment:cua_volume",
  yourCloud: "experiment:your_cloud",
  sharing: "experiment:sharing",
  webUi: "experiment:web_ui",
};

/**
 * SwiftUI: Experiments over the methods the host already routes
 * (`app.info`, `settings.choose`). About, launch at login, Devices and
 * Storage keep their operation's name on the host (`ops/webkit-pages.ts`).
 */
export function webkitSettingsOps(
  request: <T>(method: "app.info" | "settings.choose", args?: Record<string, unknown>) => Promise<T>,
): Pick<Handlers, "experiments.get" | "experiments.set"> {
  const info = async (): Promise<WkAppInfo> => (await request<WkAppInfo | null>("app.info")) ?? {};
  return {
    "experiments.get": async () => (await info()).experiments ?? {},
    "experiments.set": async ({ experiments }) => {
      const before = (await info()).experiments ?? {};
      for (const key of Object.keys(EXPERIMENT_ROWS) as (keyof Experiments)[]) {
        const on = Boolean(experiments[key]);
        if (on !== Boolean(before[key])) await request("settings.choose", { row: EXPERIMENT_ROWS[key], option: on ? "on" : "off" });
      }
      return (await info()).experiments ?? {};
    },
  };
}
