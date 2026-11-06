// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The build channel of this app: the released "Cua Spaces", or a locally built
// "Cua Spaces Local" that runs beside it and shares nothing with it (the
// Windows counterpart of the macOS app's CuaChannel::Local; the Rust names are
// libs/cua/crates/cua-home/src/channel.rs's, and test/channel.test.ts keeps
// the ones repeated here in step with it). scripts/install-local.ps1 installs
// the Local one.
//
// A process is Local when CUA_CHANNEL=local, or when its executable is
// `Cua Spaces Local.exe` (what the Local package is named). An unknown
// CUA_CHANNEL value is Stable's, as in Rust, but never silently: the Rust side
// refuses it and the app then fails to start rather than guess.
import * as path from "node:path";

export type ChannelName = "stable" | "local";

export interface Channel {
  name: ChannelName;
  isLocal: boolean;
  appName: string;
  /** The AppUserModelID (Windows groups taskbar buttons and toasts by it), and the Run entry's name. */
  appUserModelId: string;
  /** The URL scheme of the add-machine link: `cua://add/<code>` or `cua-local://add/<code>`. */
  deepLinkScheme: string;
  /** The registry key (HKCU) the app records what it put on PATH under. */
  registryKey: string;
  /** The user data folder's name under %APPDATA%. */
  userDataDirName: string;
  /** The cua home's name under the user's profile (`CUA_HOME` is pinned to it for Local). */
  homeDirName: string;
}

const STABLE: Channel = {
  name: "stable",
  isLocal: false,
  appName: "Cua Spaces",
  appUserModelId: "ai.cua.spaces.desktop",
  deepLinkScheme: "cua",
  registryKey: "Software\\ai.cua.spaces.desktop",
  userDataDirName: "Cua Spaces",
  homeDirName: ".cua",
};

const LOCAL: Channel = {
  name: "local",
  isLocal: true,
  appName: "Cua Spaces Local",
  appUserModelId: "ai.cua.spaces.desktop.local",
  deepLinkScheme: "cua-local",
  registryKey: "Software\\ai.cua.spaces.desktop.local",
  userDataDirName: "Cua Spaces Local",
  homeDirName: ".cua-local",
};

/** The Local package's executable (electron-builder names it after productName). */
export const LOCAL_EXECUTABLE = "cua spaces local.exe";

/** The channel for a `CUA_CHANNEL` value and an executable path (the environment decides first). */
export function resolveChannel(env: NodeJS.ProcessEnv = process.env, execPath: string = process.execPath): Channel {
  const set = env.CUA_CHANNEL?.trim().toLowerCase();
  if (set === "local") return LOCAL;
  if (set === "stable") return STABLE;
  if (set) throw new Error(`CUA_CHANNEL=${JSON.stringify(env.CUA_CHANNEL)} is not a Cua channel (stable or local)`);
  return path.win32.basename(execPath).toLowerCase() === LOCAL_EXECUTABLE ? LOCAL : STABLE;
}

/**
 * Local's cua home, as `CuaChannel::env_home_dir` has it: `~/.cua-local` on
 * staging (Local's default deployment), `~/.cua-local/envs/prod` on the
 * `CUA_ENV=prod` opt-in.
 */
export function localCuaHome(profile: string, env: NodeJS.ProcessEnv = process.env): string {
  const base = path.win32.join(profile, LOCAL.homeDirName);
  return (env.CUA_ENV ?? "").trim().toLowerCase() === "prod" ? path.win32.join(base, "envs", "prod") : base;
}

/** The channel of this process. */
export const channel: Channel = resolveChannel();
