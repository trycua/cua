// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Cua Volume, its first-run page and the background computer-use (driver)
 * card, as the app core lays them out.
 *
 * Hand-written mirrors of `libs/cua/crates/cua-spaces-app-core/src/`:
 * `drive_page.rs` (the Volume page), `drive_settings.rs` (the mount, the
 * storage and its bucket form), `onboarding.rs` (`DriveCard`),
 * `drive_mount_preview.rs` and `driver_preview.rs` (the two miniatures),
 * `experiments.rs` (`Experiments`) and `agents.rs` (the setup outcomes and
 * their summary). Rust is the source of truth; these are its JSON shapes.
 * The daemon's tool answers (`volume_*`) keep their snake_case.
 */

import type { SettingsOption, SettingsRow } from "./host";
import type { SpaceOs } from "./spaces";

/** Storage types shared with Settings, Storage (`drive_settings.rs`); defined in `./settings`. */
export type { DriveCheckInput, DriveS3Input, DriveStorageInput, DriveStorageUpdate, StorageAction, StorageRequest } from "./settings";

/* ---- Experiments ----------------------------------------------------------- */

/** `experiments::Experiments`: Settings, Experiments' switches (all off by default). */
export interface Experiments {
  cuaVolume?: boolean;
  yourCloud?: boolean;
  sharing?: boolean;
  webUi?: boolean;
}

export const NO_EXPERIMENTS: Required<Experiments> = { cuaVolume: false, yourCloud: false, sharing: false, webUi: false };

/* ---- The daemon's answers (volume_*) ---------------------------------------- */

/** `volume_mount_status` / `volume_mount` / `volume_unmount` (`DriveMountInput`). */
export interface DriveMountInput {
  enabled?: boolean;
  /** `off`, `mounting`, `mounted`, `needs_approval`, `error`, `unsupported`. */
  state: string;
  /** `fskit`, `nfs`, `fuse`, `none`. */
  method: string;
  path?: string | null;
  volume_name?: string;
  detail?: string | null;
  settings_url?: string | null;
  /** The Spaces that connected without a Cua Volume, and why (newer daemons). */
  volume_errors?: SpaceVolumeError[];
}

/** A Space with no Cua Volume, and the daemon's reason (`volume_errors` row). */
export interface SpaceVolumeError {
  /** The Space's name in the daemon's registry. */
  space: string;
  error: string;
}

/** `volume_requests` row. */
export interface DriveRequestInput {
  id: string;
  /** `agent:ada`. */
  principal: string;
  prefix: string;
  /** `r` or `rw`. */
  mode: string;
  reason: string;
}

/** `volume_grants` row. */
export interface DriveGrantInput {
  id: string;
  principal: string;
  prefix: string;
  mode: string;
  revoked?: boolean;
}

export interface DriveDeviceInput {
  id: string;
  name: string;
  this_device: boolean;
  last_seen_ms?: number;
  last_change_ms?: number;
}

export interface DriveConflictInput {
  path: string;
  conflict_path: string;
  winner_device: string;
  loser_device: string;
  ts_ms: number;
}

/** `volume_sync_status`. */
export interface DriveSyncInput {
  device_id: string;
  device_name: string;
  /** `live`, `off` (one device on this machine's store) or `error`. */
  feed: string;
  last_poll_ms?: number;
  pending_uploads?: number;
  conflicts?: DriveConflictInput[];
  devices?: DriveDeviceInput[];
  last_error?: string | null;
  /** The Spaces that connected without a Cua Volume, and why (newer daemons). */
  volume_errors?: SpaceVolumeError[];
}

/* ---- The Volume page (drive_page.rs) ------------------------------------------ */

/** `DriveInput`: everything the page reads. */
export interface DriveInput {
  requests?: DriveRequestInput[];
  grants?: DriveGrantInput[];
  /** Now (Unix ms), for "synced 5m ago". */
  nowMs?: number;
  mount?: DriveMountInput | null;
  sync?: DriveSyncInput | null;
  /** The home folder, to show the mount point as `~/...`. */
  home?: string | null;
}

/** `DriveRequest`: the command the shell runs. */
export type DriveRequest =
  | { kind: "load" }
  | { kind: "mount-and-reveal" }
  | { kind: "approve"; id: string }
  | { kind: "deny"; id: string }
  | { kind: "revoke"; id: string }
  | { kind: "reveal"; path: string }
  | { kind: "resolve"; path: string };

export interface DriveState {
  busy: boolean;
  error?: string | null;
  request?: DriveRequest | null;
}

export type DriveAction =
  | { type: "open-volume"; mounted: string | null }
  | { type: "approve"; id: string }
  | { type: "deny"; id: string }
  | { type: "revoke"; id: string }
  | { type: "reveal"; path: string }
  | { type: "resolve"; path: string }
  | { type: "done" }
  | { type: "failed"; error: string };

/** `persistent::LineView`: one line with up to two buttons. */
export interface LineView {
  id: string;
  text: string;
  trailing: string;
  actionLabel?: string | null;
  secondaryLabel?: string | null;
  on?: boolean | null;
}

export interface ConflictView {
  /** The file (what Resolve sends). */
  path: string;
  text: string;
  trailing: string;
  /** The losing copy to show (mounted only). */
  reveal?: string | null;
  openLabel?: string | null;
  resolveLabel: string;
}

/** `DriveView`: the page as drawn. */
export interface DriveView {
  title: string;
  requestsTitle: string;
  requests: LineView[];
  grantsTitle: string;
  grants: LineView[];
  grantsEmpty: string;
  busy: boolean;
  error?: string | null;
  request?: DriveRequest | null;
  requestText?: string | null;
  /** "Open in Finder" ("Open folder" on Linux); none where it cannot mount. */
  openLabel?: string | null;
  mountPath?: string | null;
  /** "In Finder at ~/Cua Volume", "Not mounted", "Mounting…". */
  mountLine?: string | null;
  devicesTitle: string;
  devices: LineView[];
  syncNote?: string | null;
  syncError: boolean;
  conflictsTitle: string;
  conflicts: ConflictView[];
}

/** What the Volume page's operation answers: the four tools in one read. */
export interface VolumeOverview {
  /** This machine's system (the first-run page's `drive-checked`). */
  os: SpaceOs;
  home: string | null;
  requests: DriveRequestInput[];
  grants: DriveGrantInput[];
  /** Null when the daemon could not answer. */
  mount: DriveMountInput | null;
  sync: DriveSyncInput | null;
}

/* ---- The first run's Cua Volume page (onboarding.rs) ------------------------------ */

export type StorageChoice = "local" | "s3" | "later";

/** `onboarding::DriveCard`: the Cua Volume page's card. */
export interface DriveCard {
  /** "Add Cua Volume to Finder" ("Mount Cua Volume" on Linux). */
  label: string;
  imageLabel: string;
  checked: boolean;
  enabled: boolean;
  busy: boolean;
  note?: string | null;
  error?: string | null;
  /** "Open System Settings" while macOS waits for the extension's approval. */
  settingsLabel?: string | null;
  settingsUrl?: string | null;
  storageTitle?: string | null;
  storageOptions: SettingsOption[];
  storageRows: SettingsRow[];
  storageNote?: string | null;
  storedIn?: string | null;
  mountedAt?: string | null;
  storedPath?: string | null;
  mountedPath?: string | null;
  canContinue: boolean;
}

/* ---- The two miniatures ------------------------------------------------------- */

export interface PreviewPoint {
  x: number;
  y: number;
}

export interface PreviewRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface PreviewWindow {
  frame: PreviewRect;
  titleBar: number;
  radius: number;
}

/** `driver_preview::DriverPreview`: the background computer-use card's scene. */
export interface DriverScene {
  width: number;
  height: number;
  loopMs: number;
  back: PreviewWindow;
  front: PreviewWindow;
  checkboxes: PreviewRect[];
  labels: PreviewRect[];
  lines: PreviewRect[];
  selectedLine: number;
  pointer: PreviewPoint[];
  agentPointer: PreviewPoint[];
  agentFill: string;
  agentRays: { from: PreviewPoint; to: PreviewPoint }[];
}

/** One frame of it. */
export interface DriverFrame {
  pointer: PreviewPoint;
  pressed: boolean;
  selection: number;
  agent: PreviewPoint;
  agentPressed: boolean;
  ripple: number;
  checked: number[];
}

/** `drive_mount_preview::DriveMountPreview`: the Cua Volume page's scene. */
export interface DriveScene {
  width: number;
  height: number;
  loopMs: number;
  space: PreviewWindow;
  finder: PreviewWindow;
  sidebar: PreviewRect;
  places: PreviewRect[];
  volume: PreviewRect;
  volumeIcon: PreviewRect;
  volumeLabel: string;
  volumeLabelX: number;
  fontSize: number;
  sourceIcons: PreviewRect[];
  sourceLabels: PreviewRect[];
  destIcons: PreviewRect[];
  destLabels: PreviewRect[];
}

export interface DriveFrame {
  volume: number;
  flight: PreviewPoint | null;
  arrived: number[];
}

/* ---- Agent setup outcomes (agents.rs) ---------------------------------------------- */

/** `AgentSetupOutcomeInput`: one change a setup made (`cua agents setup`). */
export interface AgentSetupOutcome {
  agents: string[];
  /** `skill` or `mcp`. */
  target: string;
  item: string;
  /** `created`, `updated`, `unchanged`, `removed`, `failed`. */
  change: string;
  detail: string;
}

/** `AgentSetupSummary`: one agent's line after setup. */
export interface AgentSetupSummary {
  /** "Codex: failed". */
  line: string;
  /** "cua MCP server configured, 2 skills installed". */
  text: string;
  /** "cua-driver: config.toml is not valid TOML". */
  failed: string[];
}
