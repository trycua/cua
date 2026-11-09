// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Settings, Devices, mirrored from the app core's `devices.rs`
 * (libs/cua/crates/cua-spaces-app-core): the relay's device list and audit
 * events in, this device's enrollment, the rows, the banner, the access log
 * and the enroll and approve sheets out.
 */

/** A device as the relay lists it (`devices::DeviceInput`). */
export interface DeviceInput {
  id: string;
  name?: string;
  /** `pending`, `enrolled`, `expired` or `revoked`. */
  state?: string;
  /** Unix seconds. */
  enrolledUntil?: number | null;
  /** Unix seconds. */
  lastSeen?: number | null;
  current?: boolean;
  platform?: string | null;
}

export interface AuditInput {
  ts: number;
  kind: string;
  device?: string | null;
  machine?: string | null;
  subject?: string | null;
  detail?: string | null;
}

export interface MachineInput {
  id: string;
  name?: string;
  confirmed?: boolean;
}

/** `devices::DevicesInput`. */
export interface DevicesInput {
  devices?: DeviceInput[];
  audit?: AuditInput[];
  localDeviceId?: string | null;
  pendingCode?: string | null;
  enforceAfter?: number | null;
  machineNames?: Record<string, string>;
  machines?: MachineInput[];
  /** The relay could not be read (this device is not enrolled, say): the
   * host's words. The page is still drawn from what is known, as the
   * SwiftUI app's Devices settings do. Not the core's. */
  readError?: string | null;
  /** This device's name as its system says it ("cua's Mac Studio"), for
   * before the relay knows it. The host's; not the core's. */
  deviceName?: string | null;
}

/** A one-time code that adds another computer to the account ("Add a
 * machine" by link or QR code; the cua SDK's `PairingCode`, camelCase).
 * `code`, `url` and the install commands are only there when just made. */
export interface PairingCode {
  /** `pc_…` (to revoke it). */
  id: string;
  /** `XXXX-XXXX-XXXX`. */
  code: string | null;
  /** `https://cua.ai/add/<code>`: the link and the QR code. */
  url: string | null;
  /** The macOS and Linux one-liner. */
  installSh: string | null;
  /** The Windows (PowerShell) one-liner. */
  installPowershell: string | null;
  state: PairingCodeState;
  /** Team the machine is added for; null for the personal account. */
  team: string | null;
  /** Unix seconds. */
  createdAt: number;
  /** Unix seconds. */
  expiresAt: number;
  /** Unix seconds, once a machine used it. */
  usedAt: number | null;
  /** The relay machine id it added. */
  machine: string | null;
}

export type PairingCodeState = "active" | "used" | "revoked" | "expired";

export type BannerTone = "info" | "warning" | "critical";
export type DeviceAction = "enroll" | "approve" | "rename" | "revoke";
export type EnrollmentKind = "enrolled" | "grace" | "needs-enrollment" | "waiting" | "due" | "revoked";

/** Why this device cannot open the account's machines (signed in, not
 * enrolled; `devices::MachineAccessNotice`): the line above a machine's
 * greyed-out Connect, its Status word, and the action that opens the
 * enroll sheet ("Enroll This Mac…"). */
export interface MachineAccessNotice {
  kind: EnrollmentKind;
  status: string;
  text: string;
  actionLabel: string;
}

export interface DeviceBanner {
  tone: BannerTone;
  text: string;
  action: DeviceAction | null;
  actionLabel: string | null;
}

export interface DeviceConfirm {
  title: string;
  message: string;
  confirmLabel: string;
  cancelLabel: string;
}

export interface UnconfirmedMachine {
  id: string;
  title: string;
  confirm: DeviceConfirm;
}

export interface DeviceRow {
  id: string;
  title: string;
  subtitle: string;
  actions: DeviceAction[];
  name: string;
  platform: string;
  detail: string;
  lastSeen: number | null;
  current: boolean;
  revokeConfirm: DeviceConfirm | null;
}

export interface ApprovalPrompt {
  deviceId: string;
  text: string;
  requiresPresence: boolean;
  name: string;
  expired: boolean;
  notifyTitle: string;
  notifyBody: string;
}

export interface ActivityRow {
  ts: number;
  text: string;
  notable: boolean;
}

export interface ThisDevice {
  kind: EnrollmentKind;
  title: string;
  /** Unix seconds; the page formats it. */
  at: number | null;
  name: string | null;
  actionLabel: string | null;
}

export interface DevicesLabels {
  title: string;
  thisDevice: string;
  devices: string;
  recent: string;
  recentEmpty: string;
  lastSeen: string;
  approve: string;
  deny: string;
  rename: string;
  revoke: string;
  renameTitle: string;
  renameConfirm: string;
  cancel: string;
  signedOut: string;
  newMachines: string;
  confirmMachine: string;
}

/** `devices::DevicesView`. */
export interface DevicesView {
  banner: DeviceBanner | null;
  enrolled: boolean;
  rows: DeviceRow[];
  approvals: ApprovalPrompt[];
  activity: ActivityRow[];
  thisDevice: ThisDevice;
  recent: ActivityRow[];
  unconfirmedMachines: UnconfirmedMachine[];
  labels: DevicesLabels;
}

/* ---- Enroll this device ---------------------------------------------------- */

export type EnrollMethod = "sign-in" | "approve";
export type EnrollPhase = "choose" | "signing-in" | "registering" | "waiting" | "enrolled" | "failed";

export interface EnrollState {
  phase: EnrollPhase;
  method: EnrollMethod | null;
  code: string | null;
  error: string | null;
}

export type EnrollAction =
  | { type: "choose"; method: EnrollMethod }
  | { type: "signed-in" }
  | { type: "registered"; enrolled: boolean; code: string | null }
  | { type: "approved" }
  | { type: "failed"; error: string }
  | { type: "back" };

export interface EnrollOption {
  method: EnrollMethod;
  title: string;
  detail: string;
}

export interface EnrollView {
  title: string;
  lede: string;
  options: EnrollOption[];
  code: string | null;
  codeHelp: string | null;
  status: string | null;
  error: string | null;
  busy: boolean;
  done: boolean;
  backLabel: string | null;
  closeLabel: string;
}

/** `devices.enroll`: this device registered with the relay. */
export interface EnrollResult {
  enrolled: boolean;
  code: string | null;
}

/* ---- Approve another device ------------------------------------------------ */

export interface ApproveSheetState {
  deviceId: string;
  name: string;
  expired: boolean;
  code: string;
  busy: boolean;
  error: string | null;
  codeExpired: boolean;
}

export type ApproveSheetAction = { type: "set-code"; code: string } | { type: "submit" } | { type: "failed"; error: string };

export interface ApproveRequest {
  code: string | null;
  deviceId: string | null;
}

export interface ApproveSheetView {
  title: string;
  message: string;
  needsCode: boolean;
  codeLabel: string;
  codePlaceholder: string;
  code: string;
  canApprove: boolean;
  approveLabel: string;
  denyLabel: string;
  denyRevokes: boolean;
  presenceReason: string;
  busy: boolean;
  error: string | null;
  request: ApproveRequest | null;
}

/** One of the account's Teams (an organization of `GET /v1/me`). */
export interface TeamChoice {
  /** The organization id (`team:<id>` shares with it). */
  id: string;
  /** Its slug. */
  alias: string;
  /** Its display name (empty: use the alias). */
  name: string;
  /** `owner`, `admin` or `member`. */
  role: string;
  /** `free`, `teams`, ...: team machines need `teams`. */
  plan: string;
}

/** `teams.list`: the account's Teams and the one it works in. */
export interface TeamsList {
  teams: TeamChoice[];
  /** The saved current team (Settings, Teams), or null. */
  currentTeamId: string | null;
}

/* ---- The account's part of the Machines page (`machines_account.rs`) --------- */

/** `machines_account::PendingKind`. */
export type PendingKind = "machine" | "sign-in";

/** A machine waiting to be added (`machines_account::PendingMachine`). */
export interface PendingMachine {
  id: string;
  kind: PendingKind;
  name: string;
  /** `macOS`, `Windows`, `Linux`, or empty. */
  os: string;
  line: string;
  /** When it last reached the relay (Unix seconds). */
  since: number | null;
  /** The question the "Add this machine?" page asks. */
  text: string;
  machine: string | null;
  device: string | null;
  expired: boolean;
  requiresPresence: boolean;
  notifyTitle: string;
  notifyBody: string;
  addLabel: string;
  notMeLabel: string;
}

/** This machine's own trust, in plain words (`machines_account::ThisMachineTrust`). */
export interface ThisMachineTrust {
  kind: EnrollmentKind;
  /** "Trusted until", "Not trusted yet", ... */
  status: string;
  /** Unix seconds; the page formats it. */
  at: number | null;
  text: string | null;
  tone: "warning" | "critical" | null;
  actionLabel: string | null;
}

/** Who reached a machine (`machines_account::AccessLine`). */
export interface AccessLine {
  ts: number;
  text: string;
  notable: boolean;
  machine: string | null;
  device: string | null;
}

/** `machines_account::MachinesAccountView`. */
export interface MachinesAccountView {
  pending: PendingMachine[];
  pendingCount: number;
  pendingTitle: string;
  thisMachine: ThisMachineTrust | null;
  access: AccessLine[];
  recentTitle: string;
  recentEmpty: string;
  readError: string | null;
}
