// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Spaces: the registry's wire rows, the Space view model the app core maps
 * them onto, and the create/power/delete state machine's types.
 *
 * Hand-written mirror, copied from the Tauri app (`apps/cua-spaces/src/
 * model/types.ts`, `model/spaces.ts`, `model/creates.ts`, `native/fleet.ts`).
 * The source of truth is Rust: `cua-spaces-app-core::model` (Space,
 * SpaceRow) and `cua-spaces-app-core::spaces::creating`. serde there is
 * camelCase, so these names are the JSON the core returns.
 */

/** `unknown`: the host has not reported a recognized operating system
 * (`model::SpaceOs::Unknown`); it is never guessed as Linux. */
export type SpaceOs = "macos" | "windows" | "linux" | "unknown";

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

/** One row of the shell's `list_spaces` (the app core's `model::SpaceRow`). */
export interface SpaceRow {
  id: string;
  /** A create the host itself runs (the SwiftUI app's pending row, one this
   * page did not start): its progress, or why it failed. The store draws it
   * as that create's row instead of a registry Space. */
  hostProgress?: SpaceProgress;
  name: string;
  /** The SDK's location word: `cloud`, `local`, `direct` or `relay`. */
  provider: SpaceProvider;
  spacesdVersion: string;
  features: string[];
  addedAt?: string;
  os?: SpaceOs;
  /** OS product or distribution spacesd reported ("Ubuntu"). */
  osName?: string;
  /** The full OS string ("Ubuntu 24.04.3 LTS"), when reported. */
  osPrettyName?: string;
  /** The image it runs and its digest, when known. */
  image?: string;
  imageDigest?: string;
  /** `container` or `vm`, and the guest's CPU architecture, when known. */
  kind?: "container" | "vm";
  arch?: string;
  reachable: boolean;
  error?: string;
  /** For a Space one of your machines provides: that machine's relay id and name. */
  host?: string;
  hostName?: string;
  /** How it turns off and on (`suspend`, `stop`); absent when it cannot. */
  power?: string;
  /** `running`, `suspended` or `stopped` as cua last left it, when known. */
  powerState?: string;
  /** For a Space in your cloud: the provider word (`aws`, `gcp`, `modal`),
   * its account and region in words ("AWS · us-west-2"), and where Delete
   * Permanently can delete it (`here`, `host:<machine>`, `elsewhere`). */
  cloud?: string;
  cloudPlace?: string;
  cloudDelete?: string;
}

export interface PendingCreate {
  id: string;
  name: string;
  os: SpaceOs;
  provider: Location | "relay";
  startedAt: number;
  phase: string;
  fraction?: number | null;
  pulled: boolean;
  permille: number;
  error?: string | null;
  spaceId?: string | null;
  image?: string | null;
  kind?: "container" | "vm" | null;
  arch?: string | null;
  emulated?: boolean;
  phaseAt?: number | null;
  /** GPU acceleration was asked for. */
  gpu?: boolean;
  /** The download's bytes so far, of how many, and its rate (bytes/s). */
  bytesDone?: number | null;
  bytesTotal?: number | null;
  bytesPerSecond?: number | null;
  /** Cancel was pressed: the row shows Cancelling until the create ends. */
  cancelling?: boolean;
  /** A create on one of your machines: its relay id (the create's
   * `host:<machine>`) and name. That machine lists the Space it is creating
   * as a record of its own; the core folds it into this row. */
  host?: string | null;
  hostName?: string | null;
}

/** One delete in flight (or done, until the registry drops the Space). */
export interface PendingDelete {
  id: string;
  startedAt: number;
  done: boolean;
}

/** Every pending create and delete. */
/** One power action in flight (or done until the registry shows it, or
 * failed until the next one). */
export interface PendingPower {
  id: string;
  /** Turning it on (else off). */
  on: boolean;
  startedAt: number;
  done: boolean;
  error?: string | null;
}

export interface CreatesState {
  pending: PendingCreate[];
  deleting?: PendingDelete[];
  powering?: PendingPower[];
}

export type CreateAction =
  | {
      type: "start";
      id: string;
      name: string;
      os: SpaceOs;
      provider: Location | "relay";
      now: number;
      /** The image (its catalog entry names the distribution, kind and
       * platforms), the kind asked for, and this Mac's CPU architecture. */
      image?: string | null;
      kind?: "container" | "vm" | null;
      hostArch?: string | null;
      /** GPU acceleration was asked for (the create's `gpu` option). */
      gpu?: boolean;
      /** The machine it runs on, when it is one of yours: its relay id and name. */
      host?: string | null;
      hostName?: string | null;
    }
  | {
      type: "progress";
      id: string;
      phase: string;
      fraction?: number | null;
      now?: number | null;
      /** An image download's bytes so far, of how many, and how fast. */
      bytesDone?: number | null;
      bytesTotal?: number | null;
      bytesPerSecond?: number | null;
    }
  /** A few times a second while a create is pending: progress within a
   * phase without a fraction advances with the time it usually takes. */
  | { type: "tick"; now: number }
  | { type: "finish"; id: string; spaceId: string }
  | { type: "fail"; id: string; error: string }
  | { type: "dismiss"; id: string }
  /** Cancel was pressed (the shell then calls `cancel_create`). */
  | { type: "cancel-start"; id: string }
  /** The cancel finished: the row goes. */
  | { type: "cancel-done"; id: string }
  /** The cancel itself failed: the row says why. */
  | { type: "cancel-fail"; id: string; error: string }
  | { type: "delete-start"; id: string; now: number }
  | { type: "delete-fail"; id: string }
  | { type: "delete-done"; id: string }
  /** The power button was pressed: the row says Suspending (and the like)
   * now; a second press while one runs does nothing. */
  | { type: "power-start"; id: string; on: boolean; now: number }
  /** The SDK's stop or start returned. */
  | { type: "power-done"; id: string }
  /** It failed: the row shows why, inline. */
  | { type: "power-fail"; id: string; error: string };

export const NO_CREATES: CreatesState = { pending: [], deleting: [], powering: [] };
