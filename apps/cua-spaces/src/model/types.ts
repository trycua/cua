// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Serializable domain model for the Cua Spaces portal.
 *
 * Everything here is plain data so it can be fixture-driven, snapshotted,
 * and tested without React or Tauri. All data in this prototype is synthetic.
 */

/** Operating system a Space runs. */
export type SpaceOs = "macos" | "windows" | "linux";

/**
 * Lifecycle status of a Space.
 *  - `local`        : the host Mac itself (never created, never billed)
 *  - `running`      : agent or user session active
 *  - `approval`     : an agent is waiting for human approval
 *  - `suspended`    : auto-suspended, resumes on demand
 *  - `provisioning` : being created
 *  - `deleting`     : being deleted (the user confirmed)
 */
export type SpaceStatus =
  | "local"
  | "running"
  | "approval"
  | "suspended"
  | "provisioning"
  | "deleting";

/** Visual scene used to draw a synthetic thumbnail (no real screen capture). */
export type ThumbnailScene =
  | "mac-desktop"
  | "windows-desktop"
  | "linux-terminal"
  | "notes"
  | "browser"
  | "blank";

/**
 * What the cua SDK reported about a registered Space (`list_spaces`). Present
 * only on Spaces from the live registry; fixture Spaces omit it.
 */
export interface SpaceSdkRef {
  /** spacesd feature names (`desktop_stream`, `window_stream`, `audio.desktop`, …). */
  features: string[];
  /** spacesd version at the last handshake. */
  spacesdVersion: string;
  /** Whether the Space answered the last bounded connect. */
  reachable: boolean;
  /** Why it did not, when it did not. */
  error?: string;
}

/**
 * Where a Space runs: the SDK's location words. `cloud` is Cua Cloud,
 * `local` this Mac, `direct` a machine added by address, `relay` a machine
 * of the signed-in account reached through the cua relay.
 */
export type SpaceProvider = "cloud" | "local" | "direct" | "relay";

/** Where new Spaces can be created (existing machines are added, not created). */
export type Location = "cloud" | "local";

/** What kind of machine a Space is. */
export type SpaceKind = "container" | "vm";

/**
 * Which engine runs it. Local: `gvisor`, `runc` (containers), `qemu`, `lume`
 * (VMs). Cloud: `gvisor` (containers), `kubevirt` (VMs). `auto` lets the SDK
 * pick for the location and image.
 */
export type Runtime = "auto" | "gvisor" | "runc" | "qemu" | "lume" | "kubevirt";

export interface Space {
  id: string;
  name: string;
  os: SpaceOs;
  status: SpaceStatus;
  /** Short human status detail, e.g. "Agent testing". */
  detail: string;
  /** Epoch milliseconds of the last time this Space was focused. */
  lastUsedAt: number;
  /** Epoch ms the Space was created: a stable anchor for the startup
   * progress so it doesn't reset when the UI reopens. */
  startedAt?: number;
  scene: ThumbnailScene;
  /** The group this Space belongs to (a cloud namespace or its location). */
  fleetId?: string;
  /** Machine size label; undefined for the local Mac. */
  size?: MachineSize;
  region?: RegionId;
  /** Where this Space runs; treated as "cloud" when omitted. */
  provider?: SpaceProvider;
  /** SDK registry facts, when the Space came from the live registry. */
  sdk?: SpaceSdkRef;
  /** OS product or distribution ("Ubuntu"), when reported: picks the OS icon. */
  osName?: string;
  /** The full OS string ("Ubuntu 24.04.3 LTS"), when reported. */
  osPrettyName?: string;
  /** The image it runs and its digest, when known. */
  image?: string;
  imageDigest?: string;
  /** Container or virtual machine, when known. */
  kind?: SpaceKind;
  /** The guest's CPU architecture (`arm64`, `amd64`), when known. */
  arch?: string;
  /** While it is being created: how far along (the app core's
   * `spaces::creating`, from the SDK's create progress). */
  progress?: SpaceProgress;
  /** For a Space one of your machines provides: that machine's relay id
   * (the sidebar nests it under the machine) and name. */
  host?: string;
  hostName?: string;
  /** Whether and how it turns off and on (the SDK's `power`), with a power
   * action in flight; absent when it cannot (a cloud Space). */
  power?: SpacePower;
  /** For a Space in your cloud: the provider word (`aws`, `gcp`, `modal`),
   * its account and region in words ("AWS · us-west-2"), and where Delete
   * Permanently can delete it (`here`, `host:<machine>`, `elsewhere`). */
  cloud?: string;
  cloudPlace?: string;
  cloudDelete?: string;
}

/** A Space's power (the app core's `model::SpacePower`). */
export interface SpacePower {
  /** How it turns off: suspended in memory or stopped with its disk. */
  control: "suspend" | "stop";
  /** It is off now. */
  off: boolean;
  /** Being turned on (`true`) or off (`false`) now. */
  turningOn?: boolean | null;
  /** Why the last power action failed, until the next one. */
  error?: string | null;
}

/** How far a Space being created has come. */
export interface SpaceProgress {
  /** The SDK's phase word (`preparing`, `pulling`, `booting`, ...). */
  phase: string;
  /** Overall progress in thousandths. */
  permille: number;
  /** The phase in words ("Starting…"), or "Failed". */
  label: string;
  /** Why the create failed. */
  error?: string;
  /** While it downloads: "4.2 of 23.9 GB · 85 MB/s · about 4 min". */
  transfer?: string;
  /** Cancel can stop it now. */
  cancellable?: boolean;
  /** Cancel was pressed; the create is being cleaned up. */
  cancelling?: boolean;
}

export type RegionId = "us-east" | "us-central" | "eu-west";
export type MachineSize = "small" | "standard" | "large";
export type AutoSuspendMinutes = 5 | 15 | 30 | 60;

export interface Region {
  id: RegionId;
  label: string;
  city: string;
}

export interface SizeOption {
  id: MachineSize;
  label: string;
  vcpu: number;
  memoryGb: number;
  /** Synthetic hourly list price in USD for a Linux computer at this size. */
  linuxHourlyUsd: number;
}

export type TemplateId = "qa-matrix" | "parallel-build" | "clean-browsers" | "custom";

export interface TemplateMember {
  name: string;
  os: SpaceOs;
  scene: ThumbnailScene;
}

export interface FleetTemplate {
  id: TemplateId;
  name: string;
  summary: string;
  description: string;
  members: TemplateMember[];
  /** Whether the user may edit the computer count. */
  adjustable: boolean;
}

export interface FleetDraft {
  templateId: TemplateId;
  region: RegionId;
  size: MachineSize;
  autoSuspend: AutoSuspendMinutes;
  /** Requested number of computers; only honoured for adjustable templates. */
  count: number;
  /** OS used for computers beyond the template's explicit members. */
  customOs: SpaceOs;
}

export interface Fleet {
  id: string;
  name: string;
  templateId: TemplateId;
  createdAt: number;
  spaceIds: string[];
  region: RegionId;
  size: MachineSize;
  autoSuspend: AutoSuspendMinutes;
}

export interface HourlyEstimate {
  computers: number;
  /** Upper bound per hour when every computer is running. */
  maxHourlyUsd: number;
  /** Per-OS breakdown for transparency. */
  breakdown: { os: SpaceOs; count: number; hourlyUsd: number }[];
}
