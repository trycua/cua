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
  /** Why it stopped without being asked to ("out of memory"), while it is off. */
  stopReason?: string | null;
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
  /** The app runs the create a second time by itself after a failure that
   * usually passes (the label says "Retrying…" while it prepares). */
  retrying?: boolean;
  /** It failed the same way twice in a row: the fix to show instead of a
   * plain Try again. */
  fix?: CreateFix;
}

/** The fix for a create that keeps failing the same way (the core's
 * `model::CreateFix`): one plain sentence and one button. */
export interface CreateFix {
  text: string;
  label: string;
  /** Copy `value` (a Terminal command), open `value` (a help page), or
   * open the failed create's details. */
  action: "copy-command" | "open-url" | "details";
  value?: string;
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
  /** Why a local Space stopped without being asked to ("out of memory"), when it did. */
  stopReason?: string;
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
  /** 1 once the app ran the create again by itself. */
  attempt?: number;
  /** The create failed and the app runs it again now: the shell starts the
   * same create (same request, same pending id) once more. */
  retryPending?: boolean;
  retriedSubclass?: string | null;
  fix?: CreateFix | null;
}

/** Creates of one image on one machine that failed for the same cause in a
 * row (the core's `creating::FailureStreak`; only the cause word). */
export interface FailureStreak {
  kind: string;
  place: string;
  image: string;
  count: number;
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
  /** The same failure in a row, per cause, machine and image. */
  streaks?: FailureStreak[];
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
  /** It failed. The core may answer by running it once more
   * (`retryPending` on its row): a failure that usually passes. */
  | { type: "fail"; id: string; error: string; errorVariant?: string; now?: number | null }
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

/**
 * How far the host's last `spaces.list` got (the SDK's `Spaces.list_known`).
 * This machine's Spaces come at once; the account's other computers as they
 * answer, so a list can be incomplete for a while (right after the daemon
 * restarts, say). An incomplete list is never "no Spaces".
 */
export interface ListStatus {
  /** The list was read at least once (before that nothing is known). */
  loaded: boolean;
  /** Some of the account's computers have not answered yet. */
  incomplete: boolean;
  /** How many computers are still being reached (0 when unknown). */
  reaching: number;
  /** The other computers' rows are the last known ones (kept across a daemon
   * restart, or from before a failed read), not yet confirmed. */
  stale: boolean;
}

/** What the Spaces page shows besides the Spaces (`spaces::ListView`). */
export interface ListView {
  /** "Create your first Space": a complete list with no Space only. */
  empty: boolean;
  /** One quiet line where the Spaces go while the list is not complete
   * ("Reaching 6 computers…"); null once it is. */
  reachingLine: string | null;
}

/** A host that lists complete lists only (`DataAdapter.listStatus` undefined). */
export const LIST_COMPLETE: ListStatus = { loaded: true, incomplete: false, reaching: 0, stale: false };
