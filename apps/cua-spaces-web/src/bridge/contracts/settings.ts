// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Settings beyond General: About (`about.rs`), launch at login
 * (`login_item.rs`), Experiments (`experiments.rs`) and Storage
 * (`drive_settings.rs`), mirrored from libs/cua/crates/cua-spaces-app-core.
 * Inputs are what a host reports; views are what the core returns.
 */

import type { LoginItemStatus, SettingsOption, SettingsSection } from "./host";
import type { DriveMountInput } from "./volume";

/** `volume_mount_status`, shared with the Volume page; defined in `./volume`. */
export type { DriveMountInput } from "./volume";

/* ---- About (`about::*`) --------------------------------------------------- */

export type UpdateChannelId = "stable" | "beta";

/** `about::AboutInput`: what the host reports about the app and its updater. */
export interface AboutInput {
  /** `macos`, `linux` or `windows`. */
  platform: string;
  version: string;
  /** May be empty. */
  build: string;
  /** This machine, for an issue report (`macOS 26.0 (arm64)`). */
  os: string;
  /** The app updates itself. */
  updater: boolean;
  autoCheck: boolean;
  autoInstall: boolean;
  channel: UpdateChannelId;
  /** The last check as the host formats a date; null: never. */
  lastCheck: string | null;
  checking: boolean;
}

export type AboutLinkId = "acknowledgements" | "privacy" | "terms" | "issue";

export interface AboutLink {
  id: AboutLinkId;
  label: string;
  /** Null: the notices the app bundles. */
  url: string | null;
}

export interface AboutUpdates {
  autoCheckLabel: string;
  autoCheck: boolean;
  autoInstallLabel: string;
  autoInstall: boolean;
  autoInstallEnabled: boolean;
  channelLabel: string;
  channels: SettingsOption[];
  channelHelp: string;
  checkLabel: string;
  checkEnabled: boolean;
  lastCheck: string;
}

/** `about::AboutView`. */
export interface AboutView {
  title: string;
  versionLine: string;
  links: AboutLink[];
  copyright: string;
  updates: AboutUpdates | null;
}

/** What `about.set` changes. */
export type AboutSetting = { key: "autoCheck"; value: boolean } | { key: "autoInstall"; value: boolean } | { key: "channel"; value: UpdateChannelId };

/* ---- Launch at login (`login_item::*`) ------------------------------------ */

/** What the host reports for launch at login (`login_item::LoginItemInput`
 * without the page's own `busy` and `error`). */
export interface LoginItemReport {
  status: LoginItemStatus;
  /** This machine provides Spaces to your other devices. */
  providesSpaces: boolean;
  /** This machine runs persistent agents. */
  runsAgents: boolean;
}

/** `login_item::LoginItemInput`. */
export interface LoginItemInput extends LoginItemReport {
  busy: boolean;
  error: string | null;
}

/* ---- Experiments (`experiments::*`) -------------------------------------- */

/** `experiments::Experiments`: one shape, defined with Volume (every switch off unless on). */
export type { Experiments } from "./volume";

/* ---- Storage (`drive_settings::*`) ---------------------------------------- */

export interface DriveS3Input {
  endpoint?: string | null;
  region: string;
  bucket: string;
  root: string;
  path_style: boolean;
}

/** `volume_storage`. */
export interface DriveStorageInput {
  backend: string;
  fs_path: string;
  s3?: DriveS3Input | null;
  has_keys: boolean;
}

/** `volume_storage_set`'s answer. */
export interface DriveCheckInput {
  ok: boolean;
  reachable: boolean;
  authorized: boolean;
  versioning: boolean;
  detail?: string | null;
  applied: boolean;
}

/** `volume_cache_stats` (the sizes Settings shows). */
export interface DriveCacheInput {
  size_bytes: number;
  capacity_bytes: number;
}

/** `drive_settings::StorageInput`. */
export interface StorageInput {
  os: "macos" | "linux" | "windows";
  home: string | null;
  storage: DriveStorageInput | null;
  mount: DriveMountInput | null;
  cache: DriveCacheInput | null;
}

/** `volume_storage_set`'s argument (the tool's snake_case). */
export interface DriveStorageUpdate {
  backend: string;
  s3: DriveS3Input | null;
  access_key_id: string | null;
  secret_access_key: string | null;
  dry_run: boolean;
}

/** `drive_settings::StorageRequest`: the command the host runs. */
export type StorageRequest =
  | { kind: "test"; update: DriveStorageUpdate }
  | { kind: "save"; update: DriveStorageUpdate }
  | { kind: "adopt"; update: DriveStorageUpdate }
  | { kind: "mount" }
  | { kind: "unmount" }
  | { kind: "reveal"; path: string }
  | { kind: "open-url"; url: string }
  | { kind: "set-cache"; capacity_bytes: number }
  | { kind: "clear-cache" };

/** `drive_settings::StorageState` (the form stays opaque to the page). */
export interface StorageState {
  form: Record<string, unknown>;
  dirty: boolean;
  busy: boolean;
  request: StorageRequest | null;
  check: DriveCheckInput | null;
  error: string | null;
  manual?: boolean;
  seen?: string | null;
}

/** `drive_settings::StorageAction`. */
export type StorageAction =
  | { type: "loaded"; storage: DriveStorageInput }
  | { type: "set-backend"; backend: string }
  | { type: "set-field"; field: string; value: string }
  | { type: "set-path-style"; on: boolean }
  | { type: "test" }
  | { type: "save" }
  | { type: "checked"; check: DriveCheckInput }
  | { type: "saved"; check: DriveCheckInput }
  | { type: "adopted"; check: DriveCheckInput }
  | { type: "show-manual"; on: boolean }
  | { type: "set-mount"; on: boolean }
  | { type: "reveal"; path: string }
  | { type: "open-url"; url: string }
  | { type: "set-cache"; capacity_bytes: number }
  | { type: "clear-cache" }
  | { type: "done" }
  | { type: "failed"; error: string };

/** The Storage section, as `storage.section` returns it (id `storage`). */
export type StorageSection = SettingsSection;
