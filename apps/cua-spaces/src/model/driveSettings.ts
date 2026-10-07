// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Settings, Storage, from the app core (`drive_settings`): where the Cua
 * Drive keeps its files (this machine or an S3-compatible bucket), the
 * drive as a Finder volume (a mount on Linux) and its block cache. The core
 * reads the daemon's `volume_storage`, `volume_mount_status` and
 * `volume_cache_stats` answers as they come and returns one Settings section
 * and the command to run; the SwiftUI app draws the same. S3 keys live only
 * in the section's state until `volume_storage_set` sends them once.
 */
import { core } from "../core";
import type { DriveMountInput } from "./persistent";
import type { SpaceOs } from "./types";
import type { SettingsSection } from "./window";

/** `volume_storage`, as the tool answers it. */
export interface DriveStorageInput {
  backend: string;
  fs_path: string;
  s3?: DriveS3Input | null;
  has_keys: boolean;
}

export interface DriveS3Input {
  endpoint?: string | null;
  region: string;
  bucket: string;
  root: string;
  path_style: boolean;
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

/** `volume_cache_stats` (Settings reads the sizes). */
export interface DriveCacheInput {
  size_bytes: number;
  capacity_bytes: number;
}

export interface StorageInput {
  os: SpaceOs;
  home: string | null;
  storage: DriveStorageInput | null;
  mount: DriveMountInput | null;
  cache: DriveCacheInput | null;
}

/** `volume_storage_set`'s argument, already the tool's snake_case. */
export interface DriveStorageUpdate {
  backend: string;
  s3: DriveS3Input | null;
  access_key_id: string | null;
  secret_access_key: string | null;
  dry_run: boolean;
}

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

export interface StorageState {
  form: Record<string, unknown>;
  dirty: boolean;
  busy: boolean;
  request: StorageRequest | null;
  check: DriveCheckInput | null;
  error: string | null;
}

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

export function storageInitial(): StorageState {
  return core("storage.initial");
}

export function reduceStorage(state: StorageState, action: StorageAction): StorageState {
  return core("storage.reduce", { state, action });
}

/** The Storage section (a Settings section, id `storage`). */
export function storageSection(input: StorageInput, state: StorageState): SettingsSection {
  return core("storage.section", { input, state });
}

/** What a row's button asks for (null: nothing). */
export function storagePress(input: StorageInput, id: string): StorageAction | null {
  return core("storage.press", { input, id });
}

/** What a row's choice asks for (null: nothing). */
export function storageChoose(id: string, option: string): StorageAction | null {
  return core("storage.choose", { id, option });
}

/** What a field's edit asks for (null: nothing). */
export function storageEdit(id: string, value: string): StorageAction | null {
  return core("storage.edit", { id, value });
}

/** One line naming a request (logs and parity). */
export function storageRequestText(request: StorageRequest): string {
  return core("storage.requestText", { request });
}
