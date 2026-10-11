// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Account, daemon, machine, onboarding, telemetry and Settings shapes.
 *
 * Hand-written mirror of the Tauri app's adapters (`apps/cua-spaces/src/
 * native/fleet.ts`, `native/host.ts`, `native/telemetry.ts`,
 * `components/desktop/NewSpaceWizard.tsx`) and of the app core's
 * `settings` module (`libs/cua/crates/cua-spaces-app-core/src/settings.rs`).
 * Rust is the source of truth; these are its JSON (camelCase) shapes.
 */

import type { AgentSetupRow } from "./agents";
import type { MachineAccessNotice } from "./devices";
import type { Location, Runtime, SpaceKind } from "./spaces";

/* ---- Account (fleet_status, begin_sign_in) ------------------------------ */

/** What starting a sign-in returns. `browser`: finish in the tab that just
 * opened. `device`: enter `userCode` at `verificationUri`. */
export interface SignInStart {
  method?: "browser" | "device";
  userCode?: string;
  verificationUri: string;
}

/** Cua Cloud account status. */
export interface FleetStatus {
  configured: boolean;
  authMode: "user" | "client-credentials" | "static-token" | "none";
  baseUrl: string;
  tokenUrl: string;
  clientId?: string;
  /** Signed-in user identity (email/subject), when a user session is active. */
  identity?: string;
  namespaces?: string[];
  probeError?: string;
}

/** The local `cua daemon` the app shares Spaces with. */
export interface DaemonStatus {
  connected: boolean;
  version?: string;
  socketPath?: string;
  loopbackUrl?: string;
  error?: string;
}

/* ---- Spaces commands ----------------------------------------------------- */

/** `create_space`'s config (the SDK's create options). */
export interface SpaceCreateConfig {
  image?: string;
  /** `local`, `cloud`, `host:<machine>`, or a connected cloud's word. */
  on?: Location | string;
  kind?: SpaceKind | "auto";
  runtime?: Runtime;
  name?: string;
  cpus?: number;
  memoryMb?: number;
  diskGb?: number;
  spacesd?: boolean;
  reuse?: boolean;
  gpu?: string;
}

/** Where new Spaces go by default (`default.on`), and where that came from. */
export interface DefaultLocation {
  value: Location | string;
  source: "env" | "config" | "default";
  env?: string;
  path: string;
}

/** `spaces:create-progress`: what a create is doing. */
export interface CreateProgress {
  pendingId: string;
  /** `preparing`, `pulling`, `creating`, `booting`, `waiting_for_services`,
   * `connecting` or `ready`. */
  phase: string;
  fraction?: number | null;
  detail: string;
  bytesDone?: number | null;
  bytesTotal?: number | null;
  bytesPerSecond?: number | null;
  /** The id the Space will have, when known. */
  space?: string | null;
}

/** What turning a Space off or on did (the SDK's `SpacePower`). */
export interface SpacePowerReport {
  space: string;
  /** `running`, `suspended` or `stopped`. */
  state: string;
  power: string;
  message: string;
}

/** `cancel_create`. */
export interface CancelOutcome {
  id: string;
  state: "cancelled" | "not_creating" | "already_created";
  message: string;
}

/* ---- Machines (list_hosts, host_status) --------------------------------- */

/** One of your machines that provides Spaces (`list_hosts`). */
export interface SpaceHost {
  id: string;
  name: string;
  via: "relay" | "direct" | string;
  online: boolean;
  os: string;
  limits: { resource: string; used: number; limit: number; reason: string }[];
}

/** A machine row as a host reports it: `SpaceHost`, plus `current` for the
 * machine the UI runs on ("This Mac"), whose `via` is `local`. */
export interface MachineRow extends SpaceHost {
  current?: boolean;
  /** Hardware or product name ("Mac mini", "ThinkCentre"), when known. */
  model?: string;
  /** This machine only: its CPU architecture as the image catalog spells it
   * (`aarch64`, `x86_64`), for the emulation warning on a Space's facts. */
  arch?: string;
  /** One line about it, in the host's words: this machine's sharing summary
   * (`host::host_summary`), or a device's state (`devices::DeviceRow.detail`). */
  detail?: string;
  /** Last session at the relay, Unix seconds (`devices::DeviceRow.lastSeen`). */
  lastSeen?: number | null;
  /** This machine only: its host status (`host_status`), when the host
   * reports one. The page draws it with the core's `host.panel`. */
  host?: HostStatus | null;
  /** One of the account's devices (`cua devices ls`), not a machine: the
   * bridge lists it once, merged into the machine it runs on
   * (`machines::merge`), or on its own when it is none of them. */
  device?: boolean;
  /** A device's state (`enrolled`, `pending`, `expired`, `revoked`). */
  deviceState?: string;
  /** The hostname the machine's cua-spacesd reported, when the app
   * reached it (matches the device named after it). */
  hostname?: string;
  /** The relay sees the machine connected now; null or absent when the
   * host does not know. */
  presence?: boolean | null;
  /** This machine only: why it cannot open the account's machines (signed
   * in, not enrolled), as the host's devices model says it. */
  accessNotice?: MachineAccessNotice | null;
}

export interface ServiceState {
  installed: boolean;
  running: boolean;
  kind: string;
  detail?: string;
}

export interface ConnectedClient {
  id: string;
  email?: string;
  name?: string;
  streams?: number;
  since?: number;
}

export interface PermissionHint {
  id: string;
  label: string;
  instructions?: string;
  settingsUrl?: string;
  granted?: boolean;
}

/** One line of the host's access log (`host::HostAccess`). */
export interface HostAccess {
  atMs: number;
  via: string;
  who: string;
  what: string;
}

/** A Space this machine provides to one of your devices (`host::HostProvidedSpace`). */
export interface HostProvidedSpace {
  relayMachine: string;
  localSpace: string;
  name: string;
  image: string;
  os: string;
  kind: string;
  createdBy: string;
  createdAtMs: number;
}

/** One line of the Spaces audit (`host::HostSpacesAudit`). */
export interface HostSpacesAudit {
  atMs: number;
  action: string;
  who: string;
  space: string;
  detail: string;
}

/** "This machine" as a host (`host_status`; trimmed to what the web UI reads). */
export interface HostStatus {
  configured: boolean;
  mode?: "relay" | "direct" | null;
  relayUrl?: string | null;
  directUrl?: string | null;
  machineId?: string | null;
  name?: string | null;
  sharing: boolean;
  service: ServiceState;
  online?: boolean | null;
  clients: ConnectedClient[];
  permissions: PermissionHint[];
  error?: string | null;
  shareDesktop?: boolean;
  provideSpaces?: boolean;
  maxSpaces?: number;
  maxMacosVms?: number;
  recentAccess?: HostAccess[];
  accessLogError?: string | null;
  providedSpaces?: HostProvidedSpace[];
  spacesAudit?: HostSpacesAudit[];
  spacesAuditError?: string | null;
  /** Relay sharing is paused until the owner signs in again. */
  pausedSignedOut?: boolean;
  /** The account this machine is registered to (relay mode): its id. */
  owner?: string | null;
  /** The owner's email, when the relay gave one. */
  ownerEmail?: string | null;
  /** Who is signed in to Cua in the app, as the host last checked
   * (`host::HostAccount`); null: nobody, or the host does not say. */
  account?: HostAccount | null;
  /** What a running setup (or Sign In) waits for, such as finishing the
   * sign-in in the browser (the SwiftUI app's `HostModel.progress`). */
  progress?: string | null;
}

/** The account signed in to Cua in the app (`host::HostAccount`). */
export interface HostAccount {
  id?: string | null;
  email?: string | null;
  display?: string | null;
}

/* ---- "This machine" page and host setup (app core `host`) ---------------- */

/** One label and value (`spaces::sidebar::Fact`, the parts drawn here). */
export interface HostFact {
  label: string;
  value: string;
}

/** A line with a time after it (`host::HostAccessRow`). */
export interface HostAccessRow {
  text: string;
  atMs: number;
}

export type HostActionId =
  | "set-up"
  | "stop-sharing"
  | "resume-sharing"
  | "remove"
  | "share-desktop"
  | "hide-desktop"
  | "provide-spaces"
  | "stop-providing-spaces"
  | "sign-in";

/** One of the two sharing settings (`host::HostToggle`). */
export interface HostToggle {
  id: string;
  label: string;
  help: string;
  on: boolean;
  enabled: boolean;
  action: HostActionId;
}

/** A panel button (`host::HostAction`). */
export interface HostAction {
  id: HostActionId;
  label: string;
  destructive: boolean;
  confirm?: { title: string; message: string; confirmLabel: string; cancelLabel: string } | null;
  /** False: drawn but not pressable now (`help` says why). Absent: enabled. */
  enabled?: boolean;
  /** The button's tooltip. */
  help?: string | null;
}

/** A permission pane still to grant (`host::PermissionRow`). */
export interface HostPermissionRow {
  id: string;
  title: string;
  help: string;
  settingsUrl?: string | null;
}

/** A way to set a machine up (`host::HostSetupChoice`). */
export interface HostSetupChoice {
  id: string;
  label: string;
  buttonLabel: string;
}

/** The "This machine" page (`host::HostPanelView`). */
export interface HostPanelView {
  title: string;
  summary: string;
  /** Why relay sharing is paused, one line in place of the summary
   * ("Paused · signed out"). */
  notice?: string | null;
  /** The notice's button ("Sign In"). */
  noticeAction?: HostAction | null;
  configured: boolean;
  facts: HostFact[];
  clientsTitle?: string | null;
  clients: string[];
  clientsEmpty?: string | null;
  recentTitle?: string | null;
  recent?: HostAccessRow[];
  /** "Show All…" when `recentAll` has more than `recent` shows. */
  recentMore?: string | null;
  /** Every recent access, repeats collapsed: the "Show All" sheet. */
  recentAll?: HostAccessRow[];
  /** The same with background probes and the owner's thumbnails too
   * (the sheet's "Include background activity"). */
  recentWithBackground?: HostAccessRow[];
  accessWarning?: string | null;
  toggles?: HostToggle[];
  limits?: string | null;
  providedTitle?: string | null;
  provided?: HostAccessRow[];
  providedEmpty?: string | null;
  activityTitle?: string | null;
  activity?: HostAccessRow[];
  /** "Show All…" when `activityAll` has more than `activity` shows. */
  activityMore?: string | null;
  /** The whole Spaces activity, repeats collapsed. */
  activityAll?: HostAccessRow[];
  activityWarning?: string | null;
  permissionsTitle?: string | null;
  permissions: HostPermissionRow[];
  openSettingsLabel: string;
  actions: HostAction[];
  intro?: string | null;
  setupChoices?: HostSetupChoice[];
}

/** A host setup form field (`host::HostFormField`). */
export interface HostFormField {
  id: string;
  label: string;
  placeholder?: string | null;
  value: string;
  toggle: boolean;
  on: boolean;
  invalid: boolean;
  advanced: boolean;
  choices?: { id: string; label: string }[];
}

/** The host setup form as drawn (`host::HostFormView`). */
export interface HostFormView {
  title: string;
  lede: string;
  fields: HostFormField[];
  advancedLabel: string;
  advancedOpen: boolean;
  backLabel: string;
  submitLabel: string;
  canSubmit: boolean;
  busy: boolean;
  error?: string | null;
}

/* ---- Launch at login (app core `login_item`) ----------------------------- */

/** What the system reports for the app as a login item (`login_item::LoginItemStatus`). */
export type LoginItemStatus = "enabled" | "notRegistered" | "requiresApproval" | "notFound";

/* ---- Onboarding, telemetry ---------------------------------------------- */

export type OnboardingMode = "client" | "host";

export interface OnboardingState {
  completed: boolean;
  mode?: OnboardingMode | null;
  installerMode?: OnboardingMode | null;
}

/** `telemetry_status`. */
export interface TelemetryView {
  enabled: boolean;
  source: string;
  sourceKind: string;
  noticeShown: boolean;
  noticeText: string;
  docsUrl: string;
}

/* ---- Settings page (app core `settings::page`) --------------------------- */

/** Where the Account section's sign-in stands (`settings::SignInPhase`). */
export type SignInPhase =
  | { kind: "idle" }
  | { kind: "starting" }
  | { kind: "waiting"; userCode?: string | null }
  | { kind: "failed"; message: string };

/** `settings::SettingsInput` (the fields the bridge fills; the rest default). */
export interface SettingsInput {
  identity?: string | null;
  apiKeyClient?: string | null;
  signIn?: SignInPhase;
  canSignOut?: boolean;
  menuBar?: boolean;
  defaultLocation?: "cloud" | "local" | "yours" | "host";
  locationLockedBy?: string | null;
  telemetry?: { enabled: boolean; lockedBy?: string | null } | null;
  /** The Keyvault's auto-wipe (none: no Keyvault section). */
  keyvaultAutoWipe?: boolean | null;
  /** "Connect to the desktop automatically" (none: no row). */
  autoConnect?: boolean | null;
  /** Which Lume macOS Spaces run on (none: no Runtimes section). */
  lumeSource?: string | null;
  /** Which engine local Linux Spaces run on (none: no Linux row). */
  linuxSource?: string | null;
  /** The coding agents (Settings, AI agents; none while detecting). */
  agents?: AgentSetupRow[] | null;
  /** "Configure all detected agents" is running. */
  agentsBusy?: boolean;
  /** The agents with a change running ("working…"). */
  agentsPending?: string[];
}

export type SettingsRowKind =
  | "text"
  | "choice"
  | "note"
  | "error"
  | "field"
  | "secret"
  | "prompt"
  | "link"
  | "toggle";

export interface SettingsOption {
  id: string;
  label: string;
  active: boolean;
}

export interface SettingsRow {
  id: string;
  kind: SettingsRowKind;
  label: string;
  value?: string | null;
  button?: string | null;
  options: SettingsOption[];
  enabled: boolean;
  help?: string | null;
  linkLabel?: string | null;
  linkUrl?: string | null;
  placeholder?: string | null;
}

export interface SettingsSection {
  id: string;
  title: string;
  button?: string | null;
  buttonEnabled: boolean;
  buttonHelp?: string | null;
  rows: SettingsRow[];
}

export interface SettingsPage {
  title: string;
  sections: SettingsSection[];
}
