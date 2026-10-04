// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The Agents, Drive and Notifications pages. Every word and decision is the
 * app core's (`agents.page*`, `drive.*`, `notifications.*`, the same the
 * SwiftUI app binds); this module only types it.
 */

import { core } from '../core';

// ---------------------------------------------------------------- agents

export interface PersistentAgentInput {
  name: string;
  harness: string;
  space: string;
  paused?: boolean;
  spaceState?: string;
  runId?: string | null;
  savedMs?: number;
  lastError?: string | null;
}

export interface DriveEntryInput {
  path: string;
  name: string;
  folder?: boolean;
  size?: number;
}

export interface FileVersionInput {
  version: string;
  modifiedMs?: number;
  deleted?: boolean;
  latest?: boolean;
}

export interface OpenFileInput {
  path: string;
  text: string;
  binary?: boolean;
  versions: FileVersionInput[];
}

export interface RoutineInput {
  id: string;
  title: string;
  label: string;
  enabled: boolean;
}

export interface ComputerGrantInput {
  agent: string;
  machine: string;
  revoked?: boolean;
}

export interface AccessAuditInput {
  tsMs: number;
  action: string;
  principal: string;
  path: string;
  detail?: string;
}

export interface AgentsInput {
  agents: PersistentAgentInput[];
  home?: DriveEntryInput[];
  file?: OpenFileInput | null;
  routines?: RoutineInput[];
  grants?: ComputerGrantInput[];
  audit?: AccessAuditInput[];
  thisMachine?: string | null;
}

export type AgentTab = 'memory' | 'routines' | 'access';
export type RoutineScheduleKind = 'every' | 'daily' | 'weekly';

export interface RoutineForm {
  title: string;
  prompt: string;
  schedule: RoutineScheduleKind;
  minutes: number;
  time: string;
  weekday: string;
}

export type AgentsRequest =
  | { kind: 'pause'; name: string }
  | { kind: 'resume'; name: string }
  | { kind: 'load'; name: string }
  | { kind: 'read-file'; path: string }
  | { kind: 'restore'; path: string; version: string }
  | {
      kind: 'add-routine';
      agent: string;
      title: string;
      prompt: string;
      every_minutes: number | null;
      daily_at: string | null;
      weekly_on: string | null;
    }
  | { kind: 'set-routine'; id: string; enabled: boolean }
  | { kind: 'remove-routine'; id: string }
  | { kind: 'allow'; agent: string; machine: string }
  | { kind: 'revoke'; agent: string; machine: string };

export interface AgentsState {
  selected: string | null;
  tab: AgentTab;
  openPath: string | null;
  form: RoutineForm;
  busy: boolean;
  error: string | null;
  request: AgentsRequest | null;
}

export type AgentsAction =
  | { type: 'select'; name: string }
  | { type: 'set-tab'; tab: AgentTab }
  | { type: 'open-file'; path: string }
  | { type: 'close-file' }
  | { type: 'pause'; name: string }
  | { type: 'resume'; name: string }
  | { type: 'restore'; version: string }
  | { type: 'set-title'; title: string }
  | { type: 'set-prompt'; prompt: string }
  | { type: 'set-schedule'; schedule: RoutineScheduleKind }
  | { type: 'set-minutes'; minutes: number }
  | { type: 'set-time'; time: string }
  | { type: 'set-weekday'; weekday: string }
  | { type: 'add-routine' }
  | { type: 'toggle-routine'; id: string }
  | { type: 'remove-routine'; id: string }
  | { type: 'allow-computer'; machine: string }
  | { type: 'revoke-computer'; machine: string }
  | { type: 'done' }
  | { type: 'failed'; error: string };

/** One line: text, trailing text, an action, a second action, an on/off. */
export interface LineView {
  id: string;
  text: string;
  trailing: string;
  actionLabel: string | null;
  secondaryLabel: string | null;
  on: boolean | null;
}

export interface AgentRowView {
  name: string;
  detail: string;
  state: string;
  actionLabel: string;
  selected: boolean;
}

export interface AgentDetailView {
  name: string;
  subtitle: string;
  tabs: { tab: AgentTab; label: string; selected: boolean }[];
  memory: LineView[];
  memoryEmpty: string;
  file: { path: string; text: string; versions: LineView[]; closeLabel: string } | null;
  routines: LineView[];
  routinesEmpty: string;
  form: RoutineForm;
  schedules: LineView[];
  canAddRoutine: boolean;
  addRoutineLabel: string;
  access: LineView[];
  accessEmpty: string;
  allowThisMachineLabel: string | null;
  audit: LineView[];
  error: string | null;
}

export interface AgentsView {
  title: string;
  rows: AgentRowView[];
  emptyText: string;
  detail: AgentDetailView | null;
  busy: boolean;
  error: string | null;
  request: AgentsRequest | null;
  requestText: string | null;
}

export function agentsInitial(): AgentsState {
  return core('agents.pageInitial');
}

export function reduceAgents(input: AgentsInput, state: AgentsState, action: AgentsAction): AgentsState {
  return core('agents.pageReduce', { input, state, action });
}

export function agentsView(input: AgentsInput, state: AgentsState, nowMs: number): AgentsView {
  return core('agents.pageView', { input, state, nowMs });
}

// ---------------------------------------------------------------- drive

export interface DriveRequestInput {
  id: string;
  principal: string;
  prefix: string;
  mode: string;
  reason?: string;
}

export interface DriveGrantInput {
  id: string;
  principal: string;
  prefix: string;
  mode: string;
  revoked?: boolean;
}

/** `volume_mount_status`, as the tool answers it (snake_case). */
export interface DriveMountInput {
  enabled: boolean;
  /** `off`, `mounting`, `mounted`, `needs_approval`, `unsupported`, `error`. */
  state: string;
  /** `fskit`, `nfs`, `fuse`, `none`. */
  method: string;
  path?: string | null;
  volume_name?: string;
  detail?: string | null;
  settings_url?: string | null;
}

/** `volume_sync_status`, as the tool answers it (snake_case). */
export interface DriveSyncInput {
  device_id: string;
  device_name: string;
  /** `live`, `off`, `error`. */
  feed: string;
  last_poll_ms: number;
  pending_uploads: number;
  conflicts: {
    path: string;
    conflict_path: string;
    winner_device: string;
    loser_device: string;
    ts_ms: number;
  }[];
  devices: {
    id: string;
    name: string;
    this_device: boolean;
    last_seen_ms: number;
    last_change_ms: number;
  }[];
  last_error?: string | null;
}

export interface DriveInput {
  requests: DriveRequestInput[];
  grants: DriveGrantInput[];
  /** Now (Unix ms), for "synced 5m ago". */
  nowMs?: number;
  /** `volume_mount_status`, once read. */
  mount?: DriveMountInput | null;
  /** `volume_sync_status`, once read. */
  sync?: DriveSyncInput | null;
  /** The home folder, to show the mount point as `~/...`. */
  home?: string | null;
}

export type DriveRequest =
  | { kind: 'load' }
  | { kind: 'mount-and-reveal' }
  | { kind: 'approve'; id: string }
  | { kind: 'deny'; id: string }
  | { kind: 'revoke'; id: string }
  | { kind: 'reveal'; path: string }
  | { kind: 'resolve'; path: string };

export interface DriveState {
  busy: boolean;
  error: string | null;
  request: DriveRequest | null;
}

export type DriveAction =
  /** The main button: reveal the mount point, mounting first when needed. */
  | { type: 'open-volume'; mounted: string | null }
  | { type: 'approve'; id: string }
  | { type: 'deny'; id: string }
  | { type: 'revoke'; id: string }
  | { type: 'reveal'; path: string }
  | { type: 'resolve'; path: string }
  | { type: 'done' }
  | { type: 'failed'; error: string };

export interface DriveView {
  title: string;
  requestsTitle: string;
  requests: LineView[];
  grantsTitle: string;
  grants: LineView[];
  grantsEmpty: string;
  busy: boolean;
  error: string | null;
  request: DriveRequest | null;
  requestText: string | null;
  /** "Open in Finder" (Linux "Open folder"), mounting first when needed;
   * null where the volume cannot mount. */
  openLabel: string | null;
  /** The mount point, when mounted. */
  mountPath: string | null;
  /** One quiet line: "In Finder at ~/Cua Volume", "Not mounted", ... */
  mountLine: string | null;
  devicesTitle: string;
  /** One line per device, this one first. */
  devices: LineView[];
  syncNote: string | null;
  syncError: boolean;
  conflictsTitle: string;
  conflicts: ConflictView[];
}

/** A file two devices wrote: the file, the other copy, Open and Resolve. */
export interface ConflictView {
  path: string;
  text: string;
  trailing: string;
  /** The copy to show in the file manager (mounted only). */
  reveal: string | null;
  openLabel: string | null;
  resolveLabel: string;
}

export function driveInitial(): DriveState {
  return core('drive.initial');
}

export function reduceDrive(state: DriveState, action: DriveAction): DriveState {
  return core('drive.reduce', { state, action });
}

export function driveView(input: DriveInput, state: DriveState): DriveView {
  return core('drive.view', { input, state });
}

// ---------------------------------------------------------------- notifications

export interface NotificationInput {
  id: string;
  atMs: number;
  agent?: string | null;
  kind: string;
  title: string;
  body: string;
  read?: boolean;
}

export interface NotificationsPlan {
  post: { id: string; title: string; body: string }[];
  seenMs: number;
}

export interface NotificationsView {
  title: string;
  rows: LineView[];
  emptyText: string;
  unread: number;
  markAllLabel: string | null;
}

export function notificationsPlan(feed: NotificationInput[], seenMs: number): NotificationsPlan {
  return core('notifications.plan', { feed, seenMs });
}

export function notificationsView(feed: NotificationInput[], nowMs: number): NotificationsView {
  return core('notifications.view', { feed, nowMs });
}

/** The daemon's `notifications_list` rows as the core reads them. */
export function feedFromTool(rows: { id: string; at_ms: number; agent?: string | null; kind: string; title: string; body: string; read?: boolean }[]): NotificationInput[] {
  return rows.map((r) => ({
    id: r.id,
    atMs: r.at_ms,
    agent: r.agent ?? null,
    kind: r.kind,
    title: r.title,
    body: r.body,
    read: Boolean(r.read),
  }));
}
