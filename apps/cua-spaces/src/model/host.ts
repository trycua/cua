// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * "This machine" in the roster and its page. The entry is always present:
 * unconfigured it offers "Set up for access"; configured it carries the
 * host's sharing state and who is connected (relay presence). Every word,
 * button and check here is the app core's (`host.*`); this module only maps
 * the shell's host status into the core's shape.
 */

import { core } from "../core";
import type { HostSettingChange, HostSetupRequest, HostStatus } from "../native/host";
import type { Space, SpaceOs } from "./types";

/** Id of the synthetic "This machine" roster entry (same as the fixture's). */
export const THIS_MACHINE_ID = "this-mac";

export function hostOs(): SpaceOs {
  const platform = typeof navigator !== "undefined" ? navigator.platform : "";
  if (/Win/i.test(platform)) return "windows";
  if (/Linux/i.test(platform)) return "linux";
  return "macos";
}

/** The shell's host status in the core's `HostState` shape. */
export function hostState(status: HostStatus) {
  return {
    configured: status.configured,
    mode: status.mode ?? null,
    relayUrl: status.relayUrl ?? null,
    directUrl: status.directUrl ?? null,
    name: status.name ?? null,
    sharing: status.sharing,
    serviceInstalled: status.service.installed,
    serviceRunning: status.service.running,
    serviceKind: status.service.kind,
    online: status.online ?? null,
    clients: status.clients.map((c) => ({
      id: c.id,
      email: c.email ?? null,
      name: c.name ?? null,
      streams: c.streams ?? null,
    })),
    permissions: status.permissions.map((p) => ({
      id: p.id,
      title: p.label,
      settingsUrl: p.settingsUrl ?? null,
      instructions: p.instructions ?? null,
      granted: Boolean(p.granted),
    })),
    error: status.error ?? null,
    recentAccess: (status.recentAccess ?? []).map((a) => ({ atMs: a.atMs, via: a.via, who: a.who, what: a.what })),
    accessLogError: status.accessLogError ?? null,
    shareDesktop: status.shareDesktop ?? true,
    provideSpaces: status.provideSpaces ?? false,
    maxSpaces: status.maxSpaces ?? 0,
    maxMacosVms: status.maxMacosVms ?? 0,
    providedSpaces: (status.providedSpaces ?? []).map((p) => ({
      relayMachine: p.relayMachine,
      localSpace: p.localSpace,
      name: p.name,
      image: p.image,
      os: p.os,
      kind: p.kind,
      createdBy: p.createdBy,
      createdAtMs: p.createdAtMs,
    })),
    spacesAudit: (status.spacesAudit ?? []).map((a) => ({
      atMs: a.atMs,
      action: a.action,
      who: a.who,
      space: a.space,
      detail: a.detail,
    })),
    spacesAuditError: status.spacesAuditError ?? null,
  };
}

function summaryInput(status: HostStatus | null) {
  return status ? core("host.summaryInput", { state: hostState(status) }) : null;
}

/** One-line status for the sidebar row's tooltip. */
export function hostSummary(status: HostStatus | null): string {
  return core("host.summary", { status: summaryInput(status) });
}

export function thisMachineSpace(status: HostStatus | null, now: number, os: SpaceOs = hostOs()): Space {
  return core("host.thisMachineSpace", { status: summaryInput(status), now, os });
}

/** The roster with "This machine" first. */
export function withThisMachine(spaces: Space[], status: HostStatus | null, now: number, os: SpaceOs = hostOs()): Space[] {
  return core("host.withThisMachine", { spaces, status: summaryInput(status), now, os });
}

export type HostActionId =
  | "set-up"
  | "stop-sharing"
  | "resume-sharing"
  | "remove"
  | "share-desktop"
  | "hide-desktop"
  | "provide-spaces"
  | "stop-providing-spaces";

/** One of the two settings, drawn as a switch (`host::HostToggle`). */
export interface HostToggle {
  id: "desktop" | "spaces";
  label: string;
  help: string;
  on: boolean;
  enabled: boolean;
  action: HostActionId;
}

/** The settings change a toggle action runs (`null` for the other actions). */
export function settingChange(id: HostActionId): HostSettingChange | null {
  return core("host.settingChange", { id });
}

export interface PermissionRow {
  id: string;
  title: string;
  help: string;
  settingsUrl: string | null;
}

/** The "This machine" page (`host::panel`). */
export interface HostPanelView {
  title: string;
  summary: string;
  configured: boolean;
  facts: { label: string; value: string }[];
  clientsTitle: string | null;
  clients: string[];
  clientsEmpty: string | null;
  /** "Recent access" when the access log has any. */
  recentTitle?: string | null;
  recent?: { text: string; atMs: number }[];
  /** Set when the access log does not verify. */
  accessWarning?: string | null;
  /** The two settings (configured only). */
  toggles?: HostToggle[];
  /** The limits, one line, while providing Spaces. */
  limits?: string | null;
  providedTitle?: string | null;
  provided?: { text: string; atMs: number }[];
  providedEmpty?: string | null;
  /** "Spaces activity": remote creates, deletes and refusals. */
  activityTitle?: string | null;
  activity?: { text: string; atMs: number }[];
  activityWarning?: string | null;
  permissionsTitle: string | null;
  permissions: PermissionRow[];
  openSettingsLabel: string;
  actions: HostAction[];
}

/** Asked before a button runs. */
export interface HostConfirm {
  title: string;
  message: string;
  confirmLabel: string;
  cancelLabel: string;
}

export interface HostAction {
  id: HostActionId;
  label: string;
  destructive: boolean;
  confirm?: HostConfirm | null;
}

export function hostPanel(status: HostStatus | null): HostPanelView {
  return core("host.panel", { state: status ? hostState(status) : null });
}

/** Permission panes still to grant. */
export function permissionRows(status: HostStatus): PermissionRow[] {
  return core("host.permissionRows", { permissions: hostState(status).permissions });
}

/** The host setup form's state (`host::HostFormState`). */
export interface HostFormState {
  name: string;
  allow: string;
  advanced: boolean;
  direct: boolean;
  listen: string;
  relayUrl: string;
  busy: boolean;
  error: string | null;
  /** A spare machine: its desktop stays private, it runs Spaces. */
  spare: boolean;
}

export type HostFormAction =
  | { type: "set-name"; name: string }
  | { type: "set-allow"; allow: string }
  | { type: "toggle-advanced" }
  | { type: "set-direct"; on: boolean }
  | { type: "set-listen"; listen: string }
  | { type: "set-relay-url"; url: string }
  | { type: "set-profile"; profile: "desktop" | "spare" }
  | { type: "submit" }
  | { type: "failed"; error: string };

export interface HostFormField {
  id: "name" | "profile" | "allow" | "direct" | "listen" | "relay";
  label: string;
  placeholder: string | null;
  value: string;
  toggle: boolean;
  on: boolean;
  invalid: boolean;
  advanced: boolean;
  /** A choice of one (`profile`): `value` is the chosen id. */
  choices?: { id: string; label: string }[];
}

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
  error: string | null;
  request: HostSetupRequest | null;
}

export function hostFormInitial(): HostFormState {
  return core("host.formInitial");
}

export function reduceHostForm(state: HostFormState, action: HostFormAction): HostFormState {
  return core("host.formReduce", { state, action });
}

export function hostFormView(state: HostFormState, identity?: string | null): HostFormView {
  return core("host.formView", { state, identity: identity ?? null });
}

/** Who a connected client is, for display. */
export function clientLabel(client: { id: string; email?: string; name?: string }): string {
  return core("host.clientLabel", { id: client.id, email: client.email ?? null, name: client.name ?? null });
}
