// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * What this Mac can run: Spaces the cua SDK runs locally (containers under
 * gVisor or runc, QEMU or Lume VMs). Backs the New Space wizard's "This Mac"
 * choice. Creating goes through `FleetBridge.createSpace({ on: "local" })`.
 * Off Tauri (browser dev / tests) `available` is simply false.
 */

import { hasTauri } from "./bridge";

/** Space on the volume a local engine writes to (the app core's `StorageVolume`). */
export interface StorageVolume {
  availableBytes: number;
  totalBytes: number;
  /** `Macintosh HD`, `Colima`, `Docker Desktop`. */
  name: string;
}

/** Where local Spaces are written and what is pulled (the app core's `LocalStorage`). */
export interface LocalStorage {
  /** Bytes the SDK keeps free on a volume. */
  reserveBytes: number;
  lume: StorageVolume | null;
  qemu: StorageVolume | null;
  container: StorageVolume | null;
  /** Catalog refs already pulled here. */
  pulled: string[];
}

/** What the SDK can run locally here (`local_status`). */
export interface LocalStatus {
  available: boolean;
  /** Ready backends from `cua runtime doctor`, e.g. ["container", "qemu", "lume"]. */
  backends: string[];
  /** The Spaces container image local container Spaces start from. */
  containerImage: string;
  /** The macOS image Lume Spaces start from, when one is configured. */
  macosImage: string | null;
  error: string | null;
  /** This Mac's architecture (`arm64`, `amd64`). */
  hostArch?: string;
  /** Free space and pulled images (the New Space wizard's Resources step). */
  storage?: LocalStorage | null;
}

export const LOCAL_UNAVAILABLE: LocalStatus = {
  available: false,
  backends: [],
  containerImage: "",
  macosImage: null,
  error: "not running in the app",
  storage: null,
};

/** Can this Mac run Spaces? */
export async function localStatus(): Promise<LocalStatus> {
  if (!hasTauri()) return LOCAL_UNAVAILABLE;
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<LocalStatus>("local_status");
}

/** Whether a macOS (Lume) Space can be created. */
export function canRunMacos(status: LocalStatus | null | undefined): boolean {
  return Boolean(status?.available && status.backends.includes("lume") && status.macosImage);
}

/** Backend names (`cua runtime doctor`, lowercased) that run containers. */
export const CONTAINER_BACKENDS = ["container", "managed", "docker", "runsc"];

/** Whether a container Space (gVisor / runc) can be created. */
export function canRunContainer(status: LocalStatus | null | undefined): boolean {
  return Boolean(
    status?.available && status.backends.some((b) => CONTAINER_BACKENDS.includes(b)),
  );
}
