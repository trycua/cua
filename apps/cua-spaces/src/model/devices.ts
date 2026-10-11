// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Devices: this device as a client of the signed-in account on the relay,
 * the account's other devices, approvals waiting for this device and the
 * account's recent access. Every word and decision is the app core's
 * (`devices.*`, the same the SwiftUI app binds); this module only types it.
 */

import { core } from "../core";

export interface DeviceInput {
  id: string;
  name: string;
  state: "pending" | "enrolled" | "expired" | "revoked" | string;
  enrolledUntil?: number | null;
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

/** One relay machine, for the "New machine" confirm badge (S5). */
export interface MachineInput {
  id: string;
  name: string;
  confirmed: boolean;
}

/** What the Devices page reads (the shell's `devices_snapshot`). */
export interface DevicesInput {
  devices: DeviceInput[];
  audit: AuditInput[];
  localDeviceId?: string | null;
  pendingCode?: string | null;
  enforceAfter?: number | null;
  machineNames?: Record<string, string>;
  machines?: MachineInput[];
}

export type DeviceAction = "enroll" | "approve" | "rename" | "revoke";

export interface DeviceConfirm {
  title: string;
  message: string;
  confirmLabel: string;
  cancelLabel: string;
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

/** A relay machine registered without an enrolled device's proof (S5). */
export interface UnconfirmedMachine {
  id: string;
  title: string;
  confirm: DeviceConfirm;
}

export interface DeviceBanner {
  tone: "info" | "warning" | "critical";
  text: string;
  action: DeviceAction | null;
  actionLabel: string | null;
}

export interface ThisDevice {
  kind: "enrolled" | "grace" | "needs-enrollment" | "waiting" | "due" | "revoked";
  title: string;
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

/** The Devices page at `nowSecs` (Unix seconds). */
export function devicesView(input: DevicesInput | null, nowSecs: number): DevicesView {
  return core("devices.view", { input, now: Math.floor(nowSecs) });
}

export function devicesLabels(): DevicesLabels {
  return core("devices.labels");
}

/** A name as typed for Rename; `null` when nothing is left. */
export function cleanDeviceName(name: string): string | null {
  return core("devices.cleanName", { name });
}

// ---- Enroll this device -------------------------------------------------

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

export interface EnrollView {
  title: string;
  lede: string;
  options: { method: EnrollMethod; title: string; detail: string }[];
  code: string | null;
  codeHelp: string | null;
  status: string | null;
  error: string | null;
  busy: boolean;
  done: boolean;
  backLabel: string | null;
  closeLabel: string;
}

export function enrollInitial(): EnrollState {
  return core("devices.enrollInitial");
}

export function reduceEnroll(state: EnrollState, action: EnrollAction): EnrollState {
  return core("devices.enrollReduce", { state, action });
}

export function enrollView(state: EnrollState): EnrollView {
  return core("devices.enrollView", { state });
}

// ---- Approve another device -------------------------------------------------

export interface ApproveSheetState {
  deviceId: string;
  name: string;
  expired: boolean;
  code: string;
  busy: boolean;
  error: string | null;
  /** The last code the relay saw expired: offers approving the waiting device of this name by id instead. */
  codeExpired: boolean;
}

export type ApproveSheetAction =
  | { type: "set-code"; code: string }
  | { type: "submit" }
  | { type: "failed"; error: string };

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
  request: { code: string | null; deviceId: string | null } | null;
}

export function approveOpen(prompt: ApprovalPrompt): ApproveSheetState {
  return core("devices.approveOpen", { prompt });
}

/**
 * `devices` is the account's current devices: once a code expires, it is
 * how the sheet finds the sole waiting device of that name to offer
 * approving by id instead.
 */
export function reduceApprove(
  state: ApproveSheetState,
  action: ApproveSheetAction,
  devices: DeviceInput[] = [],
): ApproveSheetState {
  return core("devices.approveReduce", { state, action, devices });
}

export function approveView(state: ApproveSheetState, devices: DeviceInput[] = []): ApproveSheetView {
  return core("devices.approveView", { state, devices });
}
